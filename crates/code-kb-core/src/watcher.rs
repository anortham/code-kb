use crate::debouncer::{DebouncedEvent, Debouncer, new_debouncer_opt};
use file_id::{FileId, get_file_id};
use ignore::{DirEntry, IncrementalIgnore, WalkBuilder};
use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode};
use notify::{RecursiveMode, Watcher};
use std::collections::BTreeMap;
#[cfg(test)]
use std::collections::VecDeque;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, info, trace, warn};

use crate::sync::{bounded_detail, delete_file, ensure_fresh_file, scan_workspace, update_file};
use crate::workspace::{
    Workspace, is_hard_excluded, normalize_path, permission_denied_path, strip_prefix_lossy,
    to_forward_slash, walk_error_path,
};

const EVENT_QUEUE_CAPACITY: usize = 32;
const MAX_QUEUED_EVENTS_PER_BATCH: usize = 50;
const WATCHER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const REGISTRATION_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const MAX_REGISTRATION_RETRY_INTERVAL: Duration = Duration::from_secs(30);
/// Windows watches the root recursively, with one `ReadDirectoryChangesW` handle and buffer for
/// the tree. Other platforms register each accepted folder with a nonrecursive watch.
const RECURSIVE_ROOT_WATCH: bool = cfg!(windows);
static RETIRED_WORKERS: Mutex<Vec<RetiredWorker>> = Mutex::new(Vec::new());
#[cfg(test)]
static WATCH_CALLS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
#[cfg(test)]
static HOLD_RECONCILE: Mutex<Option<(SyncSender<()>, mpsc::Receiver<()>)>> = Mutex::new(None);
#[cfg(test)]
static REUSED_FILE_ID_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static LOST_EVENTS_ROOT: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
static REMOVE_DIRECTORY_BEFORE_WATCH: Mutex<Option<PathBuf>> = Mutex::new(None);
#[cfg(test)]
struct RegistrationWalkerFault {
    after: PathBuf,
    error: ignore::Error,
    remove_directory: bool,
}
#[cfg(test)]
static REGISTRATION_WALKER_FAULTS: Mutex<VecDeque<RegistrationWalkerFault>> =
    Mutex::new(VecDeque::new());
const IGNORE_FILES: &[&str] = &[
    ".gitignore",
    ".ignore",
    ".julieignore",
    ".code-kb-ignore",
    ".codekbignore",
];

/// A recursive root watch would make `FileIdMap` hold an entry for every file of the tree,
/// ignored folders included, so Windows keeps no file IDs.
#[cfg(windows)]
type WorkspaceCache = crate::debouncer::NoCache;
#[cfg(not(windows))]
type WorkspaceCache = crate::debouncer::RecommendedCache;
type WorkspaceDebouncer = Debouncer<TracingWatcher, WorkspaceCache>;

struct TracingWatcher(notify::RecommendedWatcher);

impl Watcher for TracingWatcher {
    fn new<F: notify::EventHandler>(
        mut event_handler: F,
        config: notify::Config,
    ) -> notify::Result<Self> {
        notify::RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| {
                trace!(?event, "Received native filesystem event");
                event_handler.handle_event(event);
            },
            config,
        )
        .map(Self)
    }

    fn watch(&mut self, path: &Path, recursive_mode: RecursiveMode) -> notify::Result<()> {
        self.0.watch(path, recursive_mode)
    }

    fn unwatch(&mut self, path: &Path) -> notify::Result<()> {
        self.0.unwatch(path)
    }

    fn configure(&mut self, config: notify::Config) -> notify::Result<bool> {
        self.0.configure(config)
    }

    fn kind() -> notify::WatcherKind {
        notify::RecommendedWatcher::kind()
    }
}

fn new_debouncer<F: crate::debouncer::DebounceEventHandler>(
    timeout: Duration,
    tick_rate: Option<Duration>,
    event_handler: F,
) -> notify::Result<WorkspaceDebouncer> {
    new_debouncer_opt::<F, TracingWatcher, WorkspaceCache>(
        timeout,
        tick_rate,
        event_handler,
        WorkspaceCache::new(),
        notify::Config::default(),
    )
}

#[derive(Debug, Error)]
pub enum WatcherError {
    #[error("Failed to initialize notify watcher: {0}")]
    Notify(#[from] notify::Error),
    #[error("Failed to build gitignore filters: {0}")]
    Ignore(#[from] ignore::Error),
}

/// Active background file watcher handle. Dropping it stops the worker without waiting for
/// the step it is in; the next watcher of the same root waits for that worker to end.
pub struct WatcherHandle {
    root: String,
    failures: Arc<Mutex<WatcherFailureState>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

struct RetiredWorker {
    root: String,
    worker: JoinHandle<()>,
}

#[derive(Default)]
struct WatcherFailureState {
    generation: u64,
    failure: Option<String>,
    registration_failure: Option<String>,
}

impl WatcherHandle {
    pub fn pending_failure(&self) -> Option<(u64, String)> {
        let failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        failures
            .registration_failure
            .as_ref()
            .or(failures.failure.as_ref())
            .map(|failure| (failures.generation, failure.clone()))
    }

    pub fn clear_failure_if_unchanged(&self, generation: u64) -> bool {
        let mut failures = self
            .failures
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if failures.generation != generation || failures.registration_failure.is_some() {
            return false;
        }
        failures.failure = None;
        true
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let mut retired = RETIRED_WORKERS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            retired.retain(|retired| !retired.worker.is_finished());
            retired.push(RetiredWorker {
                root: self.root.clone(),
                worker,
            });
        }
    }
}

/// Waits for the stopped watcher workers of `root`, so an index rebuild or a new index never
/// runs beside a worker that still scans or holds the old index open.
pub(crate) fn wait_for_stopped_watchers(root: &Path) {
    join_retired_workers(&RegistrationPath::new(root).identity);
}

fn join_retired_workers(root: &str) {
    let workers = {
        let mut retired = RETIRED_WORKERS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (same_root, others): (Vec<_>, Vec<_>) = std::mem::take(&mut *retired)
            .into_iter()
            .partition(|retired| retired.root == root);
        *retired = others;
        same_root
    };
    for retired in workers {
        let _ = retired.worker.join();
    }
}

fn record_failure(failures: &Arc<Mutex<WatcherFailureState>>, failure: String) {
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    failures.generation = failures.generation.wrapping_add(1);
    if failures.failure.is_none() {
        failures.failure = Some(bounded_detail(&failure, 512));
    }
}

fn record_registration_failure(failures: &Arc<Mutex<WatcherFailureState>>, failure: String) {
    let failure = bounded_detail(&failure, 512);
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if failures.registration_failure.as_deref() != Some(&failure) {
        failures.generation = failures.generation.wrapping_add(1);
        failures.registration_failure = Some(failure);
    }
}

fn clear_registration_failure(failures: &Arc<Mutex<WatcherFailureState>>) {
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if failures.registration_failure.take().is_some() {
        failures.generation = failures.generation.wrapping_add(1);
    }
}

fn has_registration_failure(failures: &Arc<Mutex<WatcherFailureState>>) -> bool {
    failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .registration_failure
        .is_some()
}

/// Starts a filtered, debounced workspace watcher.
pub fn start_watcher(
    workspace: Workspace,
    db_path: PathBuf,
) -> Result<WatcherHandle, WatcherError> {
    let root = RegistrationPath::new(&workspace.canonical_root).identity;
    join_retired_workers(&root);
    let failures = Arc::new(Mutex::new(WatcherFailureState::default()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let (startup_sender, startup_receiver) = mpsc::sync_channel(1);
    let worker_failures = Arc::clone(&failures);
    let worker_shutdown = Arc::clone(&shutdown);
    let worker = thread::Builder::new()
        .name("code-kb-watcher".to_string())
        .spawn(move || {
            watcher_worker(
                workspace,
                db_path,
                worker_failures,
                worker_shutdown,
                startup_sender,
            );
        })
        .map_err(|error| WatcherError::Notify(notify::Error::io(error)))?;

    match startup_receiver.recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            let _ = worker.join();
            return Err(error);
        }
        Err(_) => {
            let _ = worker.join();
            return Err(WatcherError::Notify(notify::Error::generic(
                "Watcher worker exited before startup completed",
            )));
        }
    }

    Ok(WatcherHandle {
        root,
        failures,
        shutdown,
        worker: Some(worker),
    })
}

/// The watched folders by path identity, each with the registration pass that last saw it and
/// the file ID it had then, so a folder made again at a known path is watched again.
/// `watch_again` makes the next complete pass watch every known folder again, after events
/// were lost: ext4 and XFS often give a folder made again the inode number of the old one, so a
/// file ID cannot show that the native watch is gone.
#[derive(Default)]
struct RegisteredDirectories {
    paths: BTreeMap<String, RegisteredDirectory>,
    generation: u64,
    watch_again: bool,
}

struct RegisteredDirectory {
    path: PathBuf,
    generation: u64,
    file_id: Option<FileId>,
}

impl RegisteredDirectories {
    fn contains(&self, path: &RegistrationPath) -> bool {
        self.paths.contains_key(&path.identity)
    }

    fn insert(&mut self, path: RegistrationPath, generation: u64, file_id: Option<FileId>) {
        self.paths.insert(
            path.identity,
            RegisteredDirectory {
                path: path.path,
                generation,
                file_id,
            },
        );
    }

    #[cfg(all(test, not(windows)))]
    fn len(&self) -> usize {
        self.paths.len()
    }

    /// Forgets `path` and the folders below it and returns their paths.
    fn remove_tree(&mut self, path: &RegistrationPath) -> Vec<PathBuf> {
        let below = format!("{}/", path.identity.trim_end_matches('/'));
        let after_below = format!("{}0", path.identity.trim_end_matches('/'));
        let identities = std::iter::once(path.identity.clone())
            .filter(|identity| self.paths.contains_key(identity))
            .chain(
                self.paths
                    .range(below..after_below)
                    .map(|(identity, _)| identity.clone()),
            )
            .collect::<Vec<_>>();
        identities
            .into_iter()
            .filter_map(|identity| self.paths.remove(&identity))
            .map(|registered| registered.path)
            .collect()
    }
}

fn directory_file_id(path: &Path) -> Option<FileId> {
    #[cfg(test)]
    if REUSED_FILE_ID_ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        .is_some_and(|root| path.starts_with(root))
    {
        return Some(FileId::new_inode(0, 0));
    }
    get_file_id(path).ok()
}

#[cfg(test)]
fn events_are_lost(root: &Path) -> bool {
    LOST_EVENTS_ROOT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_deref()
        == Some(root)
}

#[derive(Clone, Debug)]
struct RegistrationPath {
    path: PathBuf,
    identity: String,
}

impl RegistrationPath {
    fn new(path: impl AsRef<Path>) -> Self {
        let path = normalize_path(dunce::simplified(path.as_ref()));
        let identity_path = dunce::canonicalize(&path).unwrap_or_else(|_| path.clone());
        let identity_path = normalize_path(dunce::simplified(&identity_path));
        let identity = to_forward_slash(&identity_path);
        #[cfg(windows)]
        let identity = identity.to_lowercase();
        Self { path, identity }
    }
}

impl PartialEq for RegistrationPath {
    fn eq(&self, other: &Self) -> bool {
        self.identity == other.identity
    }
}

impl Eq for RegistrationPath {}

impl Hash for RegistrationPath {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.identity.hash(state);
    }
}

fn watcher_worker(
    workspace: Workspace,
    db_path: PathBuf,
    failures: Arc<Mutex<WatcherFailureState>>,
    shutdown: Arc<AtomicBool>,
    startup_sender: SyncSender<Result<(), WatcherError>>,
) {
    let (event_sender, event_receiver) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
    let callback_failures = Arc::clone(&failures);
    let overflow = Arc::new(AtomicBool::new(false));
    let callback_overflow = Arc::clone(&overflow);
    let callback_root = workspace.canonical_root.clone();
    let mut debouncer = match new_debouncer(
        Duration::from_millis(150),
        None,
        move |result: Result<Vec<DebouncedEvent>, _>| match result {
            Ok(mut events) => {
                #[cfg(test)]
                if events_are_lost(&callback_root) {
                    callback_overflow.store(true, Ordering::Release);
                    return;
                }
                if RECURSIVE_ROOT_WATCH {
                    retain_watched_events(&mut events, &callback_root);
                }
                if events.is_empty() {
                    return;
                }
                if events.len() > MAX_QUEUED_EVENTS_PER_BATCH
                    || events.iter().any(|event| event.need_rescan())
                {
                    callback_overflow.store(true, Ordering::Release);
                    return;
                }
                trace!(?events, "Received debounced filesystem events");
                enqueue_events(&event_sender, &callback_overflow, events);
            }
            Err(errors) => {
                for error in errors {
                    warn!("File watcher error: {error:?}");
                    record_failure(&callback_failures, format!("File watcher error: {error:?}"));
                }
                callback_overflow.store(true, Ordering::Release);
            }
        },
    ) {
        Ok(debouncer) => debouncer,
        Err(error) => {
            let _ = startup_sender.send(Err(error.into()));
            return;
        }
    };

    let root = workspace.canonical_root.clone();
    if let Err(error) = watch_workspace_root(&mut debouncer, &root) {
        let _ = startup_sender.send(Err(error.into()));
        return;
    }

    let mut registrations = RegisteredDirectories::default();
    registrations.insert(RegistrationPath::new(&root), 0, directory_file_id(&root));
    if let Err(error) =
        synchronize_registrations(&mut debouncer, &workspace, &mut registrations, &shutdown)
    {
        let _ = startup_sender.send(Err(error));
        return;
    }
    if let Err(error) = scan_workspace(&workspace, &db_path, false) {
        let _ = startup_sender.send(Err(WatcherError::Notify(notify::Error::generic(&format!(
            "Failed to reconcile workspace during watcher startup: {error}"
        )))));
        return;
    }
    info!(
        "Tier 3 file watcher active on '{}' (150ms debounce)",
        workspace.canonical_root.display()
    );
    if startup_sender.send(Ok(())).is_err() {
        return;
    }

    let mut registration_retry_delay = REGISTRATION_RETRY_INTERVAL;
    let mut registration_retry_at = None;
    while !shutdown.load(Ordering::Acquire) {
        let registration_is_unhealthy = has_registration_failure(&failures);
        if !registration_is_unhealthy {
            registration_retry_at = None;
            registration_retry_delay = REGISTRATION_RETRY_INTERVAL;
        } else if registration_retry_at.is_none() {
            registration_retry_at = Some(Instant::now() + registration_retry_delay);
        }
        if registration_is_unhealthy
            && registration_retry_at.is_some_and(|retry_at| Instant::now() >= retry_at)
        {
            match synchronize_registrations(
                &mut debouncer,
                &workspace,
                &mut registrations,
                &shutdown,
            ) {
                Ok(()) if shutdown.load(Ordering::Acquire) => break,
                Ok(()) => {
                    clear_registration_failure(&failures);
                    registration_retry_delay = REGISTRATION_RETRY_INTERVAL;
                    registration_retry_at = None;
                    if let Err(error) = scan_workspace(&workspace, &db_path, false) {
                        warn!("Full scan failed after watch registration recovery: {error}");
                        record_failure(
                            &failures,
                            format!("Full scan failed after watch registration recovery: {error}"),
                        );
                    }
                }
                Err(error) => {
                    record_registration_failure(
                        &failures,
                        format!("Failed to reconcile workspace watches: {error}"),
                    );
                    registration_retry_delay = registration_retry_delay
                        .saturating_mul(2)
                        .min(MAX_REGISTRATION_RETRY_INTERVAL);
                    registration_retry_at = Some(Instant::now() + registration_retry_delay);
                }
            }
        }
        if overflow.swap(false, Ordering::AcqRel) {
            debug!("Watcher event queue overflowed; reconciling workspace");
            registrations.watch_again = true;
            reconcile_workspace(
                &mut debouncer,
                &workspace,
                &db_path,
                &mut registrations,
                &failures,
                &shutdown,
            );
            continue;
        }

        match event_receiver.recv_timeout(WATCHER_POLL_INTERVAL) {
            Ok(events) => process_events(
                events,
                &event_receiver,
                &overflow,
                &mut debouncer,
                &workspace,
                &db_path,
                &mut registrations,
                &failures,
                &shutdown,
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn workspace_walk_builder(root: &Path) -> WalkBuilder {
    let mut walker = WalkBuilder::new(root);
    walker
        .standard_filters(true)
        .hidden(false)
        .add_custom_ignore_filename(".julieignore")
        .add_custom_ignore_filename(".code-kb-ignore")
        .add_custom_ignore_filename(".codekbignore")
        .filter_entry(|entry: &DirEntry| {
            let name = entry.file_name().to_string_lossy();
            !is_hard_excluded(&name)
        });
    walker
}

/// Keeps the events whose folder a nonrecursive registration would watch, so a recursive root
/// watch passes on the same events as watches of the accepted folders.
fn retain_watched_events(events: &mut Vec<DebouncedEvent>, root: &Path) {
    let Some(mut matcher) = workspace_walk_builder(root).build_matchers().pop() else {
        return;
    };
    events.retain(|event| {
        event.need_rescan()
            || event.paths.is_empty()
            || event
                .paths
                .iter()
                .any(|path| parent_folder_is_watched(path, root, &mut matcher))
    });
}

fn parent_folder_is_watched(path: &Path, root: &Path, matcher: &mut IncrementalIgnore) -> bool {
    let absolute = normalize_path(dunce::simplified(path));
    let Some(relative) = strip_prefix_lossy(&absolute, root) else {
        return false;
    };
    let parent = relative
        .parent()
        .map(|parent| to_forward_slash(parent).trim_start_matches('/').to_string())
        .unwrap_or_default();
    if parent.is_empty() {
        return true;
    }
    !is_hard_excluded(&parent)
        && !matcher
            .matched_with_errors(Path::new(&parent), true)
            .0
            .is_ignore()
}

fn enqueue_events(
    sender: &SyncSender<Vec<DebouncedEvent>>,
    overflow: &AtomicBool,
    events: Vec<DebouncedEvent>,
) {
    if events.is_empty() {
        return;
    }
    trace!(?events, "Processing filesystem events");
    if events.len() > MAX_QUEUED_EVENTS_PER_BATCH || events.iter().any(|event| event.need_rescan())
    {
        overflow.store(true, Ordering::Release);
        return;
    }
    match sender.try_send(events) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => overflow.store(true, Ordering::Release),
        Err(TrySendError::Disconnected(_)) => {}
    }
}

fn watch_workspace_root(
    debouncer: &mut WorkspaceDebouncer,
    root: &Path,
) -> Result<(), notify::Error> {
    let mode = if RECURSIVE_ROOT_WATCH {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };
    watch_directory(debouncer, root, mode)
}

fn watch_directory(
    debouncer: &mut WorkspaceDebouncer,
    path: &Path,
    mode: RecursiveMode,
) -> Result<(), notify::Error> {
    #[cfg(test)]
    WATCH_CALLS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(path.to_path_buf());
    debouncer.watch(path, mode)
}

#[cfg(test)]
fn remove_directory_before_watch(path: &Path) {
    let mut target = REMOVE_DIRECTORY_BEFORE_WATCH
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if target.as_deref() == Some(path) {
        target.take();
        std::fs::remove_dir(path).unwrap();
    }
}

#[cfg(test)]
fn hold_reconcile() {
    let held = HOLD_RECONCILE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take();
    if let Some((entered, release)) = held {
        let _ = entered.send(());
        let _ = release.recv_timeout(Duration::from_secs(5));
    }
}

fn registration_walker(root: &Path) -> impl Iterator<Item = Result<DirEntry, ignore::Error>> {
    let walker = workspace_walk_builder(root).build();
    #[cfg(test)]
    {
        let mut walker = walker;
        let mut pending_error = None;
        std::iter::from_fn(move || {
            if let Some(error) = pending_error.take() {
                return Some(Err(error));
            }
            let result = walker.next()?;
            let fault = result.as_ref().ok().and_then(|entry| {
                let mut faults = REGISTRATION_WALKER_FAULTS
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let matches = faults.front().is_some_and(|fault| {
                    RegistrationPath::new(&fault.after) == RegistrationPath::new(entry.path())
                });
                if matches { faults.pop_front() } else { None }
            });
            if let Some(fault) = fault {
                if fault.remove_directory {
                    std::fs::remove_dir(fault.after).unwrap();
                }
                pending_error = Some(fault.error);
            }
            Some(result)
        })
    }
    #[cfg(not(test))]
    {
        walker
    }
}

fn is_missing_registered_directory_error(
    error: &ignore::Error,
    root: &RegistrationPath,
    registrations: &RegisteredDirectories,
) -> bool {
    if error.is_partial()
        || !matches!(
            error.io_error(),
            Some(error) if error.kind() == std::io::ErrorKind::NotFound
        )
    {
        return false;
    }
    let Some(path) = walk_error_path(error) else {
        return false;
    };
    let registration_path = RegistrationPath::new(path);
    registration_path != *root
        && registrations.contains(&registration_path)
        && matches!(
            std::fs::metadata(&registration_path.path),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound
        )
}

fn unreadable_subdirectory<'a>(
    error: &'a ignore::Error,
    root: &RegistrationPath,
) -> Option<&'a Path> {
    permission_denied_path(error).filter(|path| RegistrationPath::new(path) != *root)
}

fn is_permission_denied(error: &notify::Error) -> bool {
    matches!(&error.kind, notify::ErrorKind::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied)
}

fn synchronize_registrations(
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    registrations: &mut RegisteredDirectories,
    shutdown: &AtomicBool,
) -> Result<(), WatcherError> {
    let root = &workspace.canonical_root;
    let root_registration = RegistrationPath::new(root);
    let mut directory_race_retries = 0;
    'registration: loop {
        registrations.generation = registrations.generation.wrapping_add(1).max(1);
        let generation = registrations.generation;
        match registrations.paths.get_mut(&root_registration.identity) {
            Some(registered) => registered.generation = generation,
            None => {
                watch_workspace_root(debouncer, root)?;
                registrations.insert(
                    root_registration.clone(),
                    generation,
                    directory_file_id(root),
                );
            }
        }
        if RECURSIVE_ROOT_WATCH {
            return Ok(());
        }

        for result in registration_walker(root) {
            if shutdown.load(Ordering::Acquire) {
                return Ok(());
            }
            let entry = match result {
                Ok(entry) => entry,
                Err(error)
                    if directory_race_retries == 0
                        && is_missing_registered_directory_error(
                            &error,
                            &root_registration,
                            registrations,
                        ) =>
                {
                    directory_race_retries += 1;
                    continue 'registration;
                }
                Err(error) => {
                    if let Some(path) = unreadable_subdirectory(&error, &root_registration) {
                        warn!(
                            "Skipping unreadable directory '{}': {error}",
                            path.display()
                        );
                        continue;
                    }
                    return Err(error.into());
                }
            };
            let file_type = entry.file_type().ok_or_else(|| {
                notify_registration_error(format!(
                    "could not determine file type for '{}'",
                    entry.path().display()
                ))
            })?;
            if !file_type.is_dir() {
                continue;
            }

            let path = normalize_path(dunce::simplified(entry.path()));
            let registration_path = RegistrationPath::new(path.clone());
            let file_id = directory_file_id(&path);
            let watch_again = registrations.watch_again;
            match registrations.paths.get_mut(&registration_path.identity) {
                Some(registered) if file_id.is_some() && registered.file_id != file_id => {
                    debug!(
                        "Folder '{}' was made again; watching it again",
                        path.display()
                    );
                    registrations.paths.remove(&registration_path.identity);
                    let _ = debouncer.unwatch(&path);
                }
                Some(registered) if !watch_again => {
                    registered.generation = generation;
                    continue;
                }
                _ => {}
            }
            #[cfg(test)]
            remove_directory_before_watch(&path);
            match watch_directory(debouncer, &path, RecursiveMode::NonRecursive) {
                Ok(()) => {}
                Err(error)
                    if registration_path != root_registration
                        && matches!(
                            std::fs::metadata(&path),
                            Err(metadata_error)
                                if metadata_error.kind() == std::io::ErrorKind::NotFound
                        ) =>
                {
                    if directory_race_retries > 0 {
                        return Err(error.into());
                    }
                    directory_race_retries += 1;
                    continue 'registration;
                }
                Err(error)
                    if registration_path != root_registration && is_permission_denied(&error) =>
                {
                    warn!(
                        "Skipping unreadable directory '{}': {error}",
                        path.display()
                    );
                    continue;
                }
                Err(error) => return Err(error.into()),
            }
            registrations.insert(registration_path, generation, file_id);
        }
        registrations.watch_again = false;

        let stale = registrations
            .paths
            .iter()
            .filter(|(identity, registered)| {
                **identity != root_registration.identity && registered.generation != generation
            })
            .map(|(identity, registered)| (identity.clone(), registered.path.clone()))
            .collect::<Vec<_>>();
        let mut unwatch_errors = Vec::new();
        for (identity, path) in stale {
            match debouncer.unwatch(&path) {
                Ok(()) => {
                    registrations.paths.remove(&identity);
                }
                Err(error)
                    if matches!(error.kind, notify::ErrorKind::WatchNotFound) || !path.exists() =>
                {
                    registrations.paths.remove(&identity);
                }
                Err(error) => unwatch_errors.push(format!("{}: {error}", path.display())),
            }
        }
        if !unwatch_errors.is_empty() {
            return Err(notify_registration_error(format!(
                "failed to unregister {} directories: {}",
                unwatch_errors.len(),
                unwatch_errors
                    .into_iter()
                    .take(8)
                    .collect::<Vec<_>>()
                    .join("; ")
            )));
        }
        return Ok(());
    }
}

fn notify_registration_error(message: String) -> WatcherError {
    WatcherError::Notify(notify::Error::generic(&message))
}

fn reconcile_workspace(
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    db_path: &Path,
    registrations: &mut RegisteredDirectories,
    failures: &Arc<Mutex<WatcherFailureState>>,
    shutdown: &AtomicBool,
) {
    #[cfg(test)]
    hold_reconcile();
    if shutdown.load(Ordering::Acquire) {
        return;
    }
    if let Err(error) = synchronize_registrations(debouncer, workspace, registrations, shutdown) {
        warn!("Failed to reconcile workspace watch registrations: {error}");
        record_registration_failure(
            failures,
            format!("Failed to reconcile workspace watches: {error}"),
        );
    } else {
        clear_registration_failure(failures);
    }
    if shutdown.load(Ordering::Acquire) {
        return;
    }
    if let Err(error) = scan_workspace(workspace, db_path, false) {
        warn!("Full scan failed during watcher reconciliation: {error}");
        record_failure(
            failures,
            format!("Full scan failed during watcher reconciliation: {error}"),
        );
    }
}

/// The paths a removal or a move away takes from the folder tree.
fn removed_paths(event: &DebouncedEvent) -> &[PathBuf] {
    match event.kind {
        EventKind::Remove(_) => &event.paths,
        EventKind::Modify(ModifyKind::Name(RenameMode::From | RenameMode::Both)) => {
            &event.paths[..event.paths.len().min(1)]
        }
        EventKind::Modify(ModifyKind::Name(RenameMode::Any))
            if event.paths.first().is_some_and(|path| !path.exists()) =>
        {
            &event.paths[..1]
        }
        _ => &[],
    }
}

/// Drops the registrations of removed folders, so the reconcile that follows watches a folder
/// made again at the same path. The backend has already dropped their native watches.
fn forget_removed_directories(
    events: &[DebouncedEvent],
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    registrations: &mut RegisteredDirectories,
) {
    let root = RegistrationPath::new(&workspace.canonical_root);
    for path in events.iter().flat_map(removed_paths) {
        let removed = RegistrationPath::new(path);
        if removed == root {
            continue;
        }
        for path in registrations.remove_tree(&removed) {
            let _ = debouncer.unwatch(&path);
        }
    }
}

/// Refreshes changed files one at a time. Before each file it takes the batches that arrived
/// meanwhile, so a storm that reaches the queue over several debounce windows still runs one
/// bulk scan once more than `MAX_QUEUED_EVENTS_PER_BATCH` files wait. A slow extractor start
/// (Windows) otherwise costs one process for each file of the storm.
#[allow(clippy::too_many_arguments)]
fn process_events(
    events: Vec<DebouncedEvent>,
    queue: &mpsc::Receiver<Vec<DebouncedEvent>>,
    overflow: &AtomicBool,
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    db_path: &Path,
    registrations: &mut RegisteredDirectories,
    failures: &Arc<Mutex<WatcherFailureState>>,
    shutdown: &AtomicBool,
) {
    let Some(mut ignore_matcher) = workspace_walk_builder(&workspace.canonical_root)
        .build_matchers()
        .pop()
    else {
        record_failure(
            failures,
            "Failed to initialize ignore matcher for workspace root".to_string(),
        );
        return;
    };
    let mut events = events;
    let mut pending_files = BTreeMap::new();
    let mut conn = None;
    loop {
        events.extend(queue.try_iter().flatten());
        if shutdown.load(Ordering::Acquire) || overflow.load(Ordering::Acquire) {
            return;
        }
        if !events.is_empty() {
            let batch = std::mem::take(&mut events);
            let requires_reconciliation = batch
                .iter()
                .any(|event| event_requires_reconciliation(event, workspace, registrations));
            forget_removed_directories(&batch, debouncer, workspace, registrations);
            if requires_reconciliation {
                trace!("Reconciling directory events");
                reconcile_workspace(
                    debouncer,
                    workspace,
                    db_path,
                    registrations,
                    failures,
                    shutdown,
                );
                return;
            }
            collect_relevant_files(
                &batch,
                workspace,
                &mut ignore_matcher,
                failures,
                &mut pending_files,
            );
            if pending_files.len() > MAX_QUEUED_EVENTS_PER_BATCH {
                debug!(
                    "Git storm detected ({} changed files wait). Running bulk scan...",
                    pending_files.len()
                );
                if let Err(error) = scan_workspace(workspace, db_path, false) {
                    warn!("Bulk scan failed during git storm: {error}");
                    record_failure(
                        failures,
                        format!("Bulk scan failed during git storm: {error}"),
                    );
                }
                return;
            }
        }
        let Some((relative, absolute)) = pending_files.pop_first() else {
            return;
        };
        let conn = match &mut conn {
            Some(conn) => conn,
            None => match crate::db::open_read_only(db_path) {
                Ok(opened) => conn.insert(opened),
                Err(error) => {
                    warn!("Failed to open index during watcher update: {error}");
                    record_failure(
                        failures,
                        format!("Failed to open index during watcher update: {error}"),
                    );
                    return;
                }
            },
        };
        refresh_file(
            &absolute, &relative, workspace, db_path, conn, failures, shutdown,
        );
    }
}

/// Adds the workspace files that `events` name and the ignore rules accept, keyed by their
/// relative path.
fn collect_relevant_files(
    events: &[DebouncedEvent],
    workspace: &Workspace,
    ignore_matcher: &mut IncrementalIgnore,
    failures: &Arc<Mutex<WatcherFailureState>>,
    files: &mut BTreeMap<String, PathBuf>,
) {
    for event in events {
        if event.kind.is_access() {
            continue;
        }
        for path in &event.paths {
            let Some((absolute, relative)) = workspace_relative_path(path, workspace) else {
                trace!(?path, "Skipping filesystem event outside workspace");
                continue;
            };
            trace!(%relative, exists = absolute.exists(), "Processing file event path");
            if relative.is_empty() || is_hard_excluded(&relative) {
                continue;
            }
            let (matched, error) =
                ignore_matcher.matched_with_errors(Path::new(&relative), absolute.is_dir());
            if let Some(error) = error {
                warn!("Failed to load ignore rule: {error}");
                record_failure(failures, format!("Failed to load ignore rule: {error}"));
            }
            if !matched.is_ignore() {
                files.insert(relative, absolute);
            }
        }
    }
}

fn refresh_file(
    absolute: &Path,
    relative: &str,
    workspace: &Workspace,
    db_path: &Path,
    conn: &rusqlite::Connection,
    failures: &Arc<Mutex<WatcherFailureState>>,
    shutdown: &AtomicBool,
) {
    if absolute.exists() && absolute.is_file() {
        let result = match ensure_fresh_file(workspace, db_path, conn, relative) {
            Ok(_) => Ok(()),
            Err(_) => update_file(workspace, db_path, relative),
        };
        if let Err(error) = result {
            warn!("Failed to refresh '{}' from watcher: {error}", relative);
            record_failure(
                failures,
                format!("Failed to refresh '{relative}' from watcher: {error}"),
            );
        }
    } else if !absolute.exists() {
        remove_indexed_path(workspace, db_path, conn, relative, failures, shutdown);
    }
}

fn event_requires_reconciliation(
    event: &DebouncedEvent,
    workspace: &Workspace,
    registrations: &RegisteredDirectories,
) -> bool {
    if event.need_rescan() {
        return true;
    }
    if event.kind.is_access() {
        return false;
    }
    if event.paths.is_empty() {
        return true;
    }
    let mut ignore_matcher = match workspace_walk_builder(&workspace.canonical_root)
        .build_matchers()
        .pop()
    {
        Some(matcher) => matcher,
        None => return true,
    };
    let directory_event = matches!(
        event.kind,
        EventKind::Create(CreateKind::Folder) | EventKind::Remove(RemoveKind::Folder)
    );
    let unknown_event = matches!(event.kind, EventKind::Any | EventKind::Other);

    event.paths.iter().any(|path| {
        let normalized = normalize_path(dunce::simplified(path));
        let Some((absolute, relative)) = workspace_relative_path(&normalized, workspace) else {
            return false;
        };
        if is_ignore_file(&absolute) {
            return true;
        }
        if !relative.is_empty() && is_hard_excluded(&relative) {
            return false;
        }
        let (matched, _) = ignore_matcher.matched_with_errors(Path::new(&relative), true);
        if matched.is_ignore() {
            return false;
        }
        if relative.is_empty() {
            return directory_event || unknown_event;
        }
        let is_registered = registrations.contains(&RegistrationPath::new(&absolute));
        let directory_path = absolute.is_dir() || is_registered;
        directory_event || unknown_event || directory_path
    })
}

fn workspace_relative_path(path: &Path, workspace: &Workspace) -> Option<(PathBuf, String)> {
    let absolute = normalize_path(dunce::simplified(path));
    let relative = strip_prefix_lossy(&absolute, &workspace.canonical_root)?;
    let relative = to_forward_slash(relative)
        .trim_start_matches('/')
        .to_string();
    Some((absolute, relative))
}

fn is_ignore_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| IGNORE_FILES.contains(&name))
}

fn remove_indexed_path(
    workspace: &Workspace,
    db_path: &Path,
    conn: &rusqlite::Connection,
    relative: &str,
    failures: &Arc<Mutex<WatcherFailureState>>,
    shutdown: &AtomicBool,
) {
    let escaped = crate::queries::escape_like(relative);
    let pattern = format!("{escaped}/%");
    let child_paths_result = conn
        .prepare("SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\' LIMIT 51")
        .and_then(|mut statement| {
            let rows = statement.query_map([&pattern], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()
        });
    let child_paths = match child_paths_result {
        Ok(paths) => paths,
        Err(error) => {
            warn!(
                "Failed to read indexed paths for deleted '{}': {error}",
                relative
            );
            record_failure(
                failures,
                format!("Failed to read indexed paths for deleted '{relative}': {error}"),
            );
            return;
        }
    };
    if child_paths.len() > MAX_QUEUED_EVENTS_PER_BATCH {
        if let Err(error) = scan_workspace(workspace, db_path, false) {
            warn!("Bulk scan failed after deletion of '{}': {error}", relative);
            record_failure(
                failures,
                format!("Bulk scan failed after deletion of '{relative}': {error}"),
            );
        }
    } else {
        for child in child_paths {
            if shutdown.load(Ordering::Acquire) {
                return;
            }
            if let Err(error) = delete_file(workspace, db_path, &child) {
                warn!("Failed to remove deleted file '{}': {error}", child);
                record_failure(
                    failures,
                    format!("Failed to remove deleted file '{child}': {error}"),
                );
            }
        }
        if let Err(error) = delete_file(workspace, db_path, relative) {
            warn!("Failed to remove deleted file '{}': {error}", relative);
            record_failure(
                failures,
                format!("Failed to remove deleted file '{relative}': {error}"),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::time::Instant;

    /// The temp folder through its symlinks (macOS `/var` is `/private/var`), so that a test names
    /// the paths that the walk and the events name.
    fn canonical_temp(temp_dir: &tempfile::TempDir) -> PathBuf {
        dunce::canonicalize(temp_dir.path()).unwrap()
    }

    static ROOT_WATCH_FAULT_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn debounced_event(kind: EventKind, paths: &[&Path]) -> DebouncedEvent {
        let event = paths.iter().fold(notify::Event::new(kind), |event, path| {
            event.add_path(path.to_path_buf())
        });
        DebouncedEvent::new(event, Instant::now())
    }

    #[test]
    fn file_renames_and_accesses_do_not_trigger_full_reconciliation() {
        let temp_dir = crate::safe_tempdir();
        let root = temp_dir.path().to_path_buf();
        let source = root.join("src/lib.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "pub fn current() {}\n").unwrap();
        let workspace = Workspace::new(root.clone());
        let mut registrations = RegisteredDirectories::default();
        registrations.insert(RegistrationPath::new(root), 0, None);

        let renamed = debounced_event(
            EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::Both,
            )),
            &[&source],
        );
        let accessed = debounced_event(
            EventKind::Access(notify::event::AccessKind::Open(
                notify::event::AccessMode::Any,
            )),
            &[source.parent().unwrap()],
        );

        assert!(!event_requires_reconciliation(
            &renamed,
            &workspace,
            &registrations
        ));
        assert!(!event_requires_reconciliation(
            &accessed,
            &workspace,
            &registrations
        ));
        let pathless = debounced_event(EventKind::Create(CreateKind::File), &[]);
        assert!(event_requires_reconciliation(
            &pathless,
            &workspace,
            &registrations
        ));
    }

    #[test]
    fn allowed_directory_changes_reconcile_and_ignored_changes_do_not() {
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir);
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let workspace = Workspace::new(root.clone());
        let mut registrations = RegisteredDirectories::default();
        registrations.insert(RegistrationPath::new(root), 0, None);

        for (path, expected) in [
            (workspace.canonical_root.join("src"), true),
            (workspace.canonical_root.join("ignored"), false),
            (workspace.canonical_root.join("target"), false),
        ] {
            let event = debounced_event(EventKind::Create(CreateKind::Folder), &[path.as_path()]);
            assert_eq!(
                event_requires_reconciliation(&event, &workspace, &registrations),
                expected,
                "unexpected reconciliation result for {}",
                path.display()
            );
        }
    }

    #[test]
    fn full_event_queue_sets_the_coalesced_reconciliation_signal() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let overflow = AtomicBool::new(false);
        let event = || {
            debounced_event(
                EventKind::Create(CreateKind::File),
                &[Path::new("src/lib.rs")],
            )
        };

        enqueue_events(&sender, &overflow, vec![event()]);
        enqueue_events(&sender, &overflow, vec![event()]);

        assert!(overflow.load(Ordering::Acquire));
        assert_eq!(receiver.try_iter().count(), 1);
    }

    #[test]
    fn registration_failure_cannot_be_acknowledged_by_index_reconciliation() {
        let failures = Arc::new(Mutex::new(WatcherFailureState::default()));
        let handle = WatcherHandle {
            root: String::new(),
            failures: Arc::clone(&failures),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        record_registration_failure(&failures, "watch registration failed".to_string());
        let (generation, failure) = handle.pending_failure().unwrap();

        assert_eq!(failure, "watch registration failed");
        assert!(!handle.clear_failure_if_unchanged(generation));
        assert!(handle.pending_failure().is_some());

        clear_registration_failure(&failures);

        assert!(handle.pending_failure().is_none());
        assert!(!handle.clear_failure_if_unchanged(generation));
    }

    #[test]
    fn ordinary_index_failure_keeps_existing_acknowledgement_behavior() {
        let failures = Arc::new(Mutex::new(WatcherFailureState::default()));
        let handle = WatcherHandle {
            root: String::new(),
            failures: Arc::clone(&failures),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        record_failure(&failures, "index update failed".to_string());
        let generation = handle.pending_failure().unwrap().0;

        assert!(handle.clear_failure_if_unchanged(generation));
        assert!(handle.pending_failure().is_none());
    }

    #[cfg(not(windows))]
    fn watch_calls_under(root: &Path) -> Vec<PathBuf> {
        WATCH_CALLS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|path| path.starts_with(root))
            .cloned()
            .collect()
    }

    fn indexed_watcher(root: &Path, db_name: &str) -> (Workspace, PathBuf, WatcherHandle) {
        crate::sync::find_julie_extract_binary()
            .expect("julie-extract binary must be present for tests");
        let workspace = Workspace::new(root.to_path_buf());
        let db_path = root.parent().unwrap().join(db_name);
        crate::sync::scan_workspace(&workspace, &db_path, true).unwrap();
        let watcher = start_watcher(workspace.clone(), db_path.clone()).unwrap();
        (workspace, db_path, watcher)
    }

    #[cfg(not(windows))]
    #[test]
    fn removing_a_subtree_watches_no_surviving_directory_again() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let kept = root.join("kept");
        let removed_tree = root.join("removed/deep");
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(&removed_tree).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn subtree_seed() {}\n").unwrap();
        std::fs::write(
            removed_tree.join("gone.rs"),
            "pub fn removed_subtree_symbol() {}\n",
        )
        .unwrap();
        let (workspace, db_path, watcher) = indexed_watcher(&root, "subtree_removal.db");
        wait_for_indexed_symbol(&db_path, "removed_subtree_symbol", true);
        let root = workspace.canonical_root.clone();
        let watched_before = watch_calls_under(&root).len();

        std::fs::remove_dir_all(root.join("removed")).unwrap();
        wait_for_indexed_symbol(&db_path, "removed_subtree_symbol", false);
        std::fs::write(
            root.join("kept/after.rs"),
            "pub fn after_subtree_removal() {}\n",
        )
        .unwrap();
        wait_for_indexed_symbol(&db_path, "after_subtree_removal", true);

        assert_eq!(
            watch_calls_under(&root)[watched_before..],
            Vec::<PathBuf>::new()
        );
        assert!(watcher.pending_failure().is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn a_folder_made_again_at_a_registered_path_is_watched_again() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let folder = root.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let workspace = Workspace::new(root);
        let folder = workspace.canonical_root.join("folder");
        let (sender, receiver) = mpsc::channel();
        let mut debouncer = new_debouncer(
            Duration::from_millis(50),
            None,
            move |result: Result<Vec<DebouncedEvent>, _>| {
                if let Ok(events) = result {
                    let _ = sender.send(events);
                }
            },
        )
        .unwrap();
        let mut registrations = RegisteredDirectories::default();
        let shutdown = AtomicBool::new(false);
        synchronize_registrations(&mut debouncer, &workspace, &mut registrations, &shutdown)
            .unwrap();

        std::fs::rename(&folder, temp_dir.path().join("aside")).unwrap();
        std::fs::create_dir(&folder).unwrap();
        wait_for_event(&receiver, |event| {
            matches!(event.kind, EventKind::Create(_)) && event.paths == [folder.clone()]
        });
        synchronize_registrations(&mut debouncer, &workspace, &mut registrations, &shutdown)
            .unwrap();
        let new_file = folder.join("new.rs");
        std::fs::write(&new_file, "pub fn made_again() {}\n").unwrap();

        wait_for_event(&receiver, |event| event.paths == [new_file.clone()]);
    }

    #[cfg(not(windows))]
    #[test]
    fn a_folder_made_again_while_events_were_lost_is_watched_again() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir_all(root.join("sources")).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn lost_events_seed() {}\n").unwrap();
        let canonical_root = Workspace::new(root.clone()).canonical_root;
        let _reset = LostEventsReset;
        *REUSED_FILE_ID_ROOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(canonical_root.clone());
        let (_workspace, db_path, watcher) = indexed_watcher(&root, "lost_events.db");
        *LOST_EVENTS_ROOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(canonical_root.clone());

        std::fs::remove_dir(canonical_root.join("sources")).unwrap();
        std::fs::create_dir(canonical_root.join("sources")).unwrap();
        std::fs::write(
            canonical_root.join("during.rs"),
            "pub fn written_while_events_were_lost() {}\n",
        )
        .unwrap();
        wait_for_indexed_symbol(&db_path, "written_while_events_were_lost", true);
        thread::sleep(Duration::from_millis(500));
        LOST_EVENTS_ROOT
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        std::fs::write(
            canonical_root.join("sources/after.rs"),
            "pub fn written_after_lost_events() {}\n",
        )
        .unwrap();

        wait_for_indexed_symbol(&db_path, "written_after_lost_events", true);
        assert!(watcher.pending_failure().is_none());
    }

    #[cfg(not(windows))]
    struct LostEventsReset;

    #[cfg(not(windows))]
    impl Drop for LostEventsReset {
        fn drop(&mut self) {
            for hook in [&REUSED_FILE_ID_ROOT, &LOST_EVENTS_ROOT] {
                hook.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
            }
        }
    }

    #[cfg(not(windows))]
    fn wait_for_event(
        receiver: &mpsc::Receiver<Vec<DebouncedEvent>>,
        mut matches: impl FnMut(&DebouncedEvent) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            match receiver.recv_timeout(remaining) {
                Ok(events) if events.iter().any(&mut matches) => return,
                Ok(_) => {}
                Err(_) => break,
            }
        }
        panic!("the expected filesystem event did not arrive");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_removal_event_forgets_the_folder_and_the_folders_below_it() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        for folder in ["a/b", "ab", "a-b"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        let (workspace, mut debouncer, mut registrations) = registered_watcher(root);
        let root = workspace.canonical_root.clone();
        std::fs::remove_dir_all(root.join("a")).unwrap();
        let removal = debounced_event(
            EventKind::Remove(RemoveKind::Folder),
            &[root.join("a").as_path()],
        );

        forget_removed_directories(&[removal], &mut debouncer, &workspace, &mut registrations);

        let mut remaining = registrations
            .paths
            .values()
            .map(|registered| registered.path.clone())
            .collect::<Vec<_>>();
        remaining.sort();
        assert_eq!(
            remaining,
            vec![root.clone(), root.join("a-b"), root.join("ab")]
        );
    }

    #[test]
    fn a_storm_spread_over_queued_batches_runs_one_bulk_scan() {
        crate::sync::find_julie_extract_binary()
            .expect("julie-extract binary must be present for tests");
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn queued_seed() {}\n").unwrap();
        let workspace = Workspace::new(root.clone());
        let db_path = canonical_temp(&temp_dir).join("queued_storm.db");
        crate::sync::scan_workspace(&workspace, &db_path, true).unwrap();
        let mut batches = (0..3).map(|batch| {
            (0..25)
                .map(|index| {
                    let path = root.join(format!("queued_{batch}_{index:02}.rs"));
                    std::fs::write(
                        &path,
                        format!("pub fn queued_storm_{batch}_{index:02}() {{}}\n"),
                    )
                    .unwrap();
                    debounced_event(EventKind::Create(CreateKind::File), &[&path])
                })
                .collect::<Vec<_>>()
        });
        let first = batches.next().unwrap();
        let (sender, queue) = mpsc::sync_channel(EVENT_QUEUE_CAPACITY);
        let overflow = AtomicBool::new(false);
        for batch in batches {
            enqueue_events(&sender, &overflow, batch);
        }
        let mut debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            |_: crate::debouncer::DebounceEventResult| {},
        )
        .unwrap();
        let failures = Arc::new(Mutex::new(WatcherFailureState::default()));

        process_events(
            first,
            &queue,
            &overflow,
            &mut debouncer,
            &workspace,
            &db_path,
            &mut RegisteredDirectories::default(),
            &failures,
            &AtomicBool::new(false),
        );

        let conn = crate::db::open_read_only(&db_path).unwrap();
        let indexed =
            crate::queries::search_symbols(&conn, "queued_storm", None, false, 100).unwrap();
        assert_eq!(indexed.len(), 75);
        assert!(queue.try_recv().is_err());
        assert!(!overflow.load(Ordering::Acquire));
        assert!(failures.lock().unwrap().failure.is_none());
    }

    #[test]
    fn dropping_a_watcher_does_not_wait_for_its_reconcile() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn held_seed() {}\n").unwrap();
        let (workspace, db_path, watcher) = indexed_watcher(&root, "held_reconcile.db");
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release, release_receiver) = mpsc::sync_channel(1);
        *HOLD_RECONCILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((entered_sender, release_receiver));
        std::fs::create_dir(workspace.canonical_root.join("new_folder")).unwrap();
        entered.recv_timeout(Duration::from_secs(10)).unwrap();

        let dropped_at = Instant::now();
        drop(watcher);
        let drop_time = dropped_at.elapsed();
        assert!(
            drop_time < Duration::from_secs(1),
            "drop took {drop_time:?}"
        );
        let (started_sender, started) = mpsc::channel();
        let next_workspace = workspace.clone();
        let next_db_path = db_path.clone();
        thread::spawn(move || {
            let _ = started_sender.send(start_watcher(next_workspace, next_db_path));
        });
        let waited_for_old_worker = started.recv_timeout(Duration::from_millis(500)).is_err();
        let _ = release.send(());
        let next = started
            .recv_timeout(Duration::from_secs(30))
            .unwrap()
            .unwrap();

        assert!(waited_for_old_worker);
        drop(next);
    }

    #[test]
    fn an_index_rebuild_waits_for_a_stopped_watcher_of_its_root() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn rebuild_seed() {}\n").unwrap();
        let (workspace, db_path, watcher) = indexed_watcher(&root, "rebuild_wait.db");
        let (entered_sender, entered) = mpsc::sync_channel(1);
        let (release, release_receiver) = mpsc::sync_channel(1);
        *HOLD_RECONCILE
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some((entered_sender, release_receiver));
        std::fs::create_dir(workspace.canonical_root.join("new_folder")).unwrap();
        entered.recv_timeout(Duration::from_secs(10)).unwrap();
        drop(watcher);

        let (rebuilt_sender, rebuilt) = mpsc::channel();
        thread::spawn(move || {
            let _ = rebuilt_sender.send(crate::sync::ensure_index_matches_extractor(
                &workspace, &db_path, "999.0.0",
            ));
        });
        let rebuilt_while_held = rebuilt.recv_timeout(Duration::from_millis(500)).ok();
        let _ = release.send(());

        assert!(rebuilt_while_held.is_none());
        assert!(
            rebuilt
                .recv_timeout(Duration::from_secs(60))
                .unwrap()
                .unwrap()
        );
    }

    #[test]
    fn a_recursive_root_watch_keeps_only_events_of_watched_folders() {
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir_all(root.join("src/deep")).unwrap();
        std::fs::create_dir_all(root.join("ignored/inner")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        let root = Workspace::new(root).canonical_root;
        let kept = [
            root.join("src/deep/lib.rs"),
            root.join("src/lib.rs"),
            root.join(".gitignore"),
            root.join("ignored"),
            root.join("target"),
        ];
        let dropped = [
            root.join("ignored/inner/lib.rs"),
            root.join("ignored/.gitignore"),
            root.join("target/debug/build.rs"),
        ];
        let mut events = kept
            .iter()
            .chain(&dropped)
            .map(|path| debounced_event(EventKind::Create(CreateKind::Any), &[path.as_path()]))
            .collect::<Vec<_>>();
        events.push(debounced_event(EventKind::Create(CreateKind::Any), &[]));

        retain_watched_events(&mut events, &root);

        let mut paths = events
            .iter()
            .map(|event| event.paths.first().cloned())
            .collect::<Vec<_>>();
        let mut expected = kept.iter().cloned().map(Some).collect::<Vec<_>>();
        expected.push(None);
        paths.sort();
        expected.sort();
        assert_eq!(paths, expected);
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_skips_a_directory_the_walk_may_not_read() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fault_reset = WalkerFaultReset;
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let unreadable = root.join("unreadable");
        std::fs::create_dir_all(unreadable.join("inner")).unwrap();
        std::fs::create_dir_all(root.join("readable")).unwrap();
        let (workspace, mut debouncer, mut registrations) = registered_watcher(root);
        REGISTRATION_WALKER_FAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(RegistrationWalkerFault {
                after: unreadable.clone(),
                error: queued_walker_io_error(
                    Some(unreadable.clone()),
                    std::io::ErrorKind::PermissionDenied,
                ),
                remove_directory: false,
            });

        let result = synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        );

        assert!(result.is_ok(), "{result:?}");
        assert!(registrations.contains(&RegistrationPath::new(
            workspace.canonical_root.join("readable")
        )));
    }

    #[cfg(unix)]
    #[test]
    fn registration_skips_a_directory_without_read_permission() {
        use std::os::unix::fs::PermissionsExt;
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let locked = root.join("locked");
        std::fs::create_dir_all(locked.join("inner")).unwrap();
        std::fs::create_dir_all(root.join("open")).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let workspace = Workspace::new(root);
        let mut debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            |_result: Result<Vec<DebouncedEvent>, _>| {},
        )
        .unwrap();
        let mut registrations = RegisteredDirectories::default();

        let result = synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        );
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(result.is_ok(), "{result:?}");
        assert!(registrations.contains(&RegistrationPath::new(
            workspace.canonical_root.join("open")
        )));
        // inotify needs read permission to watch a folder; FSEvents does not.
        #[cfg(target_os = "linux")]
        assert!(!registrations.contains(&RegistrationPath::new(
            workspace.canonical_root.join("locked")
        )));
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_rewalks_when_a_discovered_directory_disappears() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let disappearing = root.join("disappearing");
        std::fs::create_dir_all(&disappearing).unwrap();
        let workspace = Workspace::new(root);
        let mut debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            |_result: Result<Vec<DebouncedEvent>, _>| {},
        )
        .unwrap();
        let mut registrations = RegisteredDirectories::default();
        *REMOVE_DIRECTORY_BEFORE_WATCH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(disappearing.clone());

        let result = synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        );

        assert!(
            result.is_ok(),
            "the registration walk should recover after a discovered directory disappears: {result:?}"
        );
        assert!(!disappearing.exists());
        assert_eq!(registrations.generation, 2);
        assert_eq!(registrations.len(), 1);
    }

    fn queued_walker_io_error(path: Option<PathBuf>, kind: std::io::ErrorKind) -> ignore::Error {
        let error = ignore::Error::Io(std::io::Error::from(kind));
        let error = match path {
            Some(path) => ignore::Error::WithPath {
                path,
                err: Box::new(error),
            },
            None => error,
        };
        ignore::Error::WithDepth {
            depth: 1,
            err: Box::new(error),
        }
    }

    #[cfg(not(windows))]
    fn registered_watcher(root: PathBuf) -> (Workspace, WorkspaceDebouncer, RegisteredDirectories) {
        let workspace = Workspace::new(root);
        let mut debouncer = new_debouncer(
            Duration::from_millis(150),
            None,
            |_result: Result<Vec<DebouncedEvent>, _>| {},
        )
        .unwrap();
        let mut registrations = RegisteredDirectories::default();
        synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        )
        .unwrap();
        (workspace, debouncer, registrations)
    }

    #[cfg(not(windows))]
    struct WalkerFaultReset;

    #[cfg(not(windows))]
    impl Drop for WalkerFaultReset {
        fn drop(&mut self) {
            REGISTRATION_WALKER_FAULTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clear();
            *REMOVE_DIRECTORY_BEFORE_WATCH
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_rewalks_after_a_registered_walker_entry_disappears() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fault_reset = WalkerFaultReset;
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let disappearing = root.join("disappearing");
        std::fs::create_dir_all(&disappearing).unwrap();
        let (workspace, mut debouncer, mut registrations) = registered_watcher(root);
        assert!(registrations.contains(&RegistrationPath::new(&disappearing)));
        REGISTRATION_WALKER_FAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(RegistrationWalkerFault {
                after: disappearing.clone(),
                error: queued_walker_io_error(
                    Some(disappearing.clone()),
                    std::io::ErrorKind::NotFound,
                ),
                remove_directory: true,
            });
        let generation = registrations.generation;

        let result = synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        );

        assert!(
            result.is_ok(),
            "a disappeared registered directory should trigger a bounded rewalk: {result:?}"
        );
        assert!(!disappearing.exists());
        assert!(registrations.generation >= generation + 2);
        assert_eq!(registrations.len(), 1);
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_walker_errors_outside_the_missing_registered_directory_race_stay_errors() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fault_reset = WalkerFaultReset;
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let registered = root.join("registered");
        std::fs::create_dir_all(&registered).unwrap();
        let (workspace, mut debouncer, mut registrations) = registered_watcher(root.clone());

        let partial_not_found = ignore::Error::WithDepth {
            depth: 1,
            err: Box::new(ignore::Error::WithPath {
                path: registered.clone(),
                err: Box::new(ignore::Error::Partial(vec![ignore::Error::Io(
                    std::io::Error::from(std::io::ErrorKind::NotFound),
                )])),
            }),
        };
        let foreign = root.join("foreign");
        for (label, after, error, remove_directory) in [
            (
                "root path",
                root.clone(),
                queued_walker_io_error(Some(root.clone()), std::io::ErrorKind::NotFound),
                false,
            ),
            (
                "root permission error",
                root.clone(),
                queued_walker_io_error(Some(root.clone()), std::io::ErrorKind::PermissionDenied),
                false,
            ),
            (
                "still-present directory",
                registered.clone(),
                queued_walker_io_error(Some(registered.clone()), std::io::ErrorKind::NotFound),
                false,
            ),
            (
                "pathless error",
                registered.clone(),
                queued_walker_io_error(None, std::io::ErrorKind::NotFound),
                false,
            ),
            (
                "foreign directory",
                registered.clone(),
                queued_walker_io_error(Some(foreign), std::io::ErrorKind::NotFound),
                false,
            ),
            ("partial error", registered.clone(), partial_not_found, true),
        ] {
            REGISTRATION_WALKER_FAULTS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push_back(RegistrationWalkerFault {
                    after,
                    error,
                    remove_directory,
                });
            let generation = registrations.generation;

            let result = synchronize_registrations(
                &mut debouncer,
                &workspace,
                &mut registrations,
                &AtomicBool::new(false),
            );

            assert!(result.is_err(), "{label} should remain an error");
            assert_eq!(registrations.generation, generation + 1, "{label}");
        }
    }

    #[test]
    fn missing_root_walker_errors_are_not_recoverable() {
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        std::fs::create_dir(&root).unwrap();
        let root_registration = RegistrationPath::new(&root);
        let mut registrations = RegisteredDirectories::default();
        registrations.insert(root_registration.clone(), 1, None);
        std::fs::remove_dir(&root).unwrap();
        let error = queued_walker_io_error(Some(root), std::io::ErrorKind::NotFound);

        assert!(!is_missing_registered_directory_error(
            &error,
            &root_registration,
            &registrations,
        ));
    }

    #[cfg(not(windows))]
    #[test]
    fn registration_walker_race_shares_the_single_retry_with_watch_failures() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fault_reset = WalkerFaultReset;
        let temp_dir = crate::safe_tempdir();
        let root = canonical_temp(&temp_dir).join("project");
        let registered = root.join("z_registered");
        let removed_before_watch = root.join("a_unregistered");
        std::fs::create_dir_all(&registered).unwrap();
        let (workspace, mut debouncer, mut registrations) = registered_watcher(root);
        std::fs::create_dir(&removed_before_watch).unwrap();
        *REMOVE_DIRECTORY_BEFORE_WATCH
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(removed_before_watch.clone());
        REGISTRATION_WALKER_FAULTS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push_back(RegistrationWalkerFault {
                after: registered.clone(),
                error: queued_walker_io_error(
                    Some(registered.clone()),
                    std::io::ErrorKind::NotFound,
                ),
                remove_directory: true,
            });

        let result = synchronize_registrations(
            &mut debouncer,
            &workspace,
            &mut registrations,
            &AtomicBool::new(false),
        );

        assert!(
            result.is_err(),
            "two directory races should exceed the shared retry"
        );
        assert!(!registered.exists());
        assert!(!removed_before_watch.exists());
        assert_eq!(registrations.generation, 3);
    }

    #[cfg(not(windows))]
    fn wait_for_indexed_symbol(db_path: &Path, name: &str, expected: bool) {
        wait_until(
            &format!("symbol {name} reaches indexed state {expected}"),
            || {
                let Ok(conn) = crate::db::open_read_only(db_path) else {
                    return false;
                };
                crate::queries::search_symbols(&conn, name, None, false, 10).is_ok_and(|symbols| {
                    symbols.iter().any(|symbol| symbol.name == name) == expected
                })
            },
        );
    }

    #[cfg(not(windows))]
    fn wait_until(description: &str, mut predicate: impl FnMut() -> bool) {
        for _ in 0..100 {
            if predicate() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
        panic!("{description} did not become true before timeout");
    }

    #[cfg(windows)]
    #[test]
    fn registration_path_equality_and_hash_share_windows_identity() {
        let temp_dir = crate::safe_tempdir();
        let path = temp_dir.path().to_path_buf();
        let trailing = PathBuf::from(format!("{}\\", path.display()));
        let verbatim = PathBuf::from(format!(r"\\?\{}", path.display()));
        let different_case = PathBuf::from(path.to_string_lossy().to_uppercase());
        let mut paths = std::collections::HashSet::new();

        paths.insert(RegistrationPath::new(&path));
        paths.insert(RegistrationPath::new(&trailing));
        paths.insert(RegistrationPath::new(&verbatim));
        paths.insert(RegistrationPath::new(&different_case));

        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn test_drive_root_leading_slash_trimmed() {
        let raw_rel = Path::new("/src/components/button.rs");
        let rel_str = to_forward_slash(raw_rel);
        let trimmed = rel_str.trim_start_matches('/').to_string();
        assert_eq!(trimmed, "src/components/button.rs");
        assert!(!trimmed.starts_with('/'));

        let raw_rel_win = Path::new("\\src\\components\\button.rs");
        let rel_str_win = to_forward_slash(raw_rel_win);
        let trimmed_win = rel_str_win.trim_start_matches('/').to_string();
        assert_eq!(trimmed_win, "src/components/button.rs");
        assert!(!trimmed_win.starts_with('/'));
    }

    #[test]
    fn test_child_files_query_pattern_escaped() {
        let dir_rel = "src/components";
        let escaped = dir_rel
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}/%");
        assert_eq!(pattern, "src/components/%");

        let dir_with_wildcards = "src/foo_bar%baz";
        let escaped2 = dir_with_wildcards
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern2 = format!("{escaped2}/%");
        assert_eq!(pattern2, "src/foo\\_bar\\%baz/%");
    }
}
