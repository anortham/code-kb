use ignore::{DirEntry, WalkBuilder};
use notify::event::{CreateKind, EventKind, RemoveKind};
use notify::{RecursiveMode, Watcher};
use notify_debouncer_full::{DebouncedEvent, Debouncer, RecommendedCache, new_debouncer_opt};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::{debug, info, trace, warn};

use crate::sync::{delete_file, ensure_fresh_file, scan_workspace, update_file};
use crate::workspace::{
    Workspace, is_hard_excluded, normalize_path, strip_prefix_lossy, to_forward_slash,
};

const EVENT_QUEUE_CAPACITY: usize = 32;
const MAX_QUEUED_EVENTS_PER_BATCH: usize = 50;
const WATCHER_POLL_INTERVAL: Duration = Duration::from_millis(100);
const REGISTRATION_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const MAX_REGISTRATION_RETRY_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(test)]
static FAIL_NEXT_ROOT_WATCH: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
static ROOT_WATCH_FAILURE_COUNT: AtomicUsize = AtomicUsize::new(0);
#[cfg(test)]
static REMOVE_DIRECTORY_BEFORE_WATCH: Mutex<Option<PathBuf>> = Mutex::new(None);
const IGNORE_FILES: &[&str] = &[
    ".gitignore",
    ".ignore",
    ".julieignore",
    ".code-kb-ignore",
    ".codekbignore",
];

type WorkspaceDebouncer = Debouncer<TracingWatcher, RecommendedCache>;

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

fn new_debouncer<F: notify_debouncer_full::DebounceEventHandler>(
    timeout: Duration,
    tick_rate: Option<Duration>,
    event_handler: F,
) -> notify::Result<WorkspaceDebouncer> {
    new_debouncer_opt::<F, TracingWatcher, RecommendedCache>(
        timeout,
        tick_rate,
        event_handler,
        RecommendedCache::new(),
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

/// Active background file watcher handle.
pub struct WatcherHandle {
    failures: Arc<Mutex<WatcherFailureState>>,
    shutdown: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
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
            let _ = worker.join();
        }
    }
}

fn record_failure(failures: &Arc<Mutex<WatcherFailureState>>, failure: String) {
    let mut failures = failures
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    failures.generation = failures.generation.wrapping_add(1);
    if failures.failure.is_none() {
        failures.failure = Some(failure.chars().take(512).collect());
    }
}

fn record_registration_failure(failures: &Arc<Mutex<WatcherFailureState>>, failure: String) {
    let failure = failure.chars().take(512).collect::<String>();
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
        failures,
        shutdown,
        worker: Some(worker),
    })
}

#[derive(Default)]
struct RegisteredDirectories {
    paths: std::collections::HashMap<RegistrationPath, u64>,
    generation: u64,
    root_watch_requires_registration: bool,
    root_unwatch_requires_retry: bool,
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
    let mut debouncer = match new_debouncer(
        Duration::from_millis(150),
        None,
        move |result: Result<Vec<DebouncedEvent>, _>| match result {
            Ok(events) if events.is_empty() => {}
            Ok(events)
                if events.len() > MAX_QUEUED_EVENTS_PER_BATCH
                    || events.iter().any(|event| event.need_rescan()) =>
            {
                callback_overflow.store(true, Ordering::Release);
            }
            Ok(events) => {
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
    registrations.paths.insert(RegistrationPath::new(root), 0);
    if let Err(error) = synchronize_registrations(&mut debouncer, &workspace, &mut registrations) {
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
            match synchronize_registrations(&mut debouncer, &workspace, &mut registrations) {
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
            reconcile_workspace(
                &mut debouncer,
                &workspace,
                &db_path,
                &mut registrations,
                &failures,
            );
            continue;
        }

        match event_receiver.recv_timeout(WATCHER_POLL_INTERVAL) {
            Ok(events) => process_events(
                events,
                &mut debouncer,
                &workspace,
                &db_path,
                &mut registrations,
                &failures,
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
    #[cfg(test)]
    if FAIL_NEXT_ROOT_WATCH.swap(false, Ordering::AcqRel) {
        ROOT_WATCH_FAILURE_COUNT.fetch_add(1, Ordering::AcqRel);
        return Err(notify::Error::generic(
            "Injected workspace root watch failure",
        ));
    }
    debouncer.watch(root, RecursiveMode::NonRecursive)
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

fn synchronize_registrations(
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    registrations: &mut RegisteredDirectories,
) -> Result<(), WatcherError> {
    let root = &workspace.canonical_root;
    let root_registration = RegistrationPath::new(root);
    let mut root_resets = 0;
    let mut directory_race_retries = 0;
    'registration: loop {
        registrations.generation = registrations.generation.wrapping_add(1).max(1);
        let generation = registrations.generation;
        if registrations.root_unwatch_requires_retry {
            match debouncer.unwatch(root) {
                Ok(()) => {
                    registrations.paths.clear();
                    registrations.root_watch_requires_registration = true;
                    registrations.root_unwatch_requires_retry = false;
                }
                Err(error)
                    if matches!(error.kind, notify::ErrorKind::WatchNotFound)
                        || (!root.exists()
                            && matches!(error.kind, notify::ErrorKind::PathNotFound)) =>
                {
                    registrations.paths.remove(&root_registration);
                    registrations.root_watch_requires_registration = true;
                    registrations.root_unwatch_requires_retry = false;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if registrations.root_watch_requires_registration
            || !registrations.paths.contains_key(&root_registration)
        {
            watch_workspace_root(debouncer, root)?;
            registrations
                .paths
                .insert(root_registration.clone(), generation);
            registrations.root_watch_requires_registration = false;
        } else if let Some(registered_generation) = registrations.paths.get_mut(&root_registration)
        {
            *registered_generation = generation;
        }
        let mut walker = workspace_walk_builder(root).build();

        for result in &mut walker {
            let entry = result.map_err(WatcherError::Ignore)?;
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
            if let Some(registered_generation) = registrations.paths.get_mut(&registration_path) {
                *registered_generation = generation;
            } else {
                #[cfg(test)]
                remove_directory_before_watch(&path);
                match debouncer.watch(&path, RecursiveMode::NonRecursive) {
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
                    Err(error) => return Err(error.into()),
                }
                registrations.paths.insert(registration_path, generation);
            }
        }

        let stale = registrations
            .paths
            .iter()
            .filter(|(path, registered_generation)| {
                **path != root_registration && **registered_generation != generation
            })
            .map(|(path, _)| (*path).clone())
            .collect::<Vec<_>>();
        let mut unwatch_errors = Vec::new();
        let mut missing_watches = Vec::new();
        for path in stale {
            match debouncer.unwatch(&path.path) {
                Ok(()) => {
                    registrations.paths.remove(&path);
                }
                Err(error)
                    if matches!(error.kind, notify::ErrorKind::WatchNotFound)
                        || (!path.path.exists()
                            && matches!(error.kind, notify::ErrorKind::PathNotFound)) =>
                {
                    missing_watches.push(path);
                }
                Err(error) => unwatch_errors.push(format!("{}: {error}", path.path.display())),
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
        if missing_watches.is_empty() {
            return Ok(());
        }
        if root_resets > 0 {
            return Err(notify_registration_error(format!(
                "{} directories lost their watches during registration repair",
                missing_watches.len()
            )));
        }

        // The debouncer retains cache roots when the native watch was already removed.
        match debouncer.unwatch(root) {
            Ok(()) => {
                registrations.paths.clear();
                registrations.root_watch_requires_registration = true;
            }
            Err(error) if matches!(error.kind, notify::ErrorKind::WatchNotFound) => {
                registrations.paths.remove(&root_registration);
                registrations.root_watch_requires_registration = true;
                return Err(error.into());
            }
            Err(error)
                if !root.exists() && matches!(error.kind, notify::ErrorKind::PathNotFound) =>
            {
                registrations.paths.remove(&root_registration);
                registrations.root_watch_requires_registration = true;
                return Err(error.into());
            }
            Err(error) => {
                registrations.root_unwatch_requires_retry = true;
                return Err(error.into());
            }
        }
        watch_workspace_root(debouncer, root)?;
        registrations
            .paths
            .insert(root_registration.clone(), generation);
        registrations.root_watch_requires_registration = false;
        root_resets += 1;
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
) {
    if let Err(error) = synchronize_registrations(debouncer, workspace, registrations) {
        warn!("Failed to reconcile workspace watch registrations: {error}");
        record_registration_failure(
            failures,
            format!("Failed to reconcile workspace watches: {error}"),
        );
    } else {
        clear_registration_failure(failures);
    }
    if let Err(error) = scan_workspace(workspace, db_path, false) {
        warn!("Full scan failed during watcher reconciliation: {error}");
        record_failure(
            failures,
            format!("Full scan failed during watcher reconciliation: {error}"),
        );
    }
}

fn process_events(
    events: Vec<DebouncedEvent>,
    debouncer: &mut WorkspaceDebouncer,
    workspace: &Workspace,
    db_path: &Path,
    registrations: &mut RegisteredDirectories,
    failures: &Arc<Mutex<WatcherFailureState>>,
) {
    if events.is_empty() {
        return;
    }
    if events
        .iter()
        .any(|event| event_requires_reconciliation(event, workspace, registrations))
    {
        trace!("Reconciling directory events");
        reconcile_workspace(debouncer, workspace, db_path, registrations, failures);
        return;
    }

    let mut ignore_matcher = match workspace_walk_builder(&workspace.canonical_root)
        .build_matchers()
        .pop()
    {
        Some(matcher) => matcher,
        None => {
            record_failure(
                failures,
                "Failed to initialize ignore matcher for workspace root".to_string(),
            );
            return;
        }
    };

    let mut relevant_files = Vec::new();
    for event in &events {
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
                relevant_files.push((absolute, relative));
            }
        }
    }

    if relevant_files.is_empty() {
        return;
    }

    relevant_files.sort_by(|left, right| left.1.cmp(&right.1));
    relevant_files.dedup_by(|left, right| left.1 == right.1);
    if relevant_files.len() > MAX_QUEUED_EVENTS_PER_BATCH {
        debug!(
            "Git storm detected ({} files changed in window). Running bulk scan...",
            relevant_files.len()
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

    let conn = match crate::db::open_read_only(db_path) {
        Ok(conn) => conn,
        Err(error) => {
            warn!("Failed to open index during watcher update: {error}");
            record_failure(
                failures,
                format!("Failed to open index during watcher update: {error}"),
            );
            return;
        }
    };
    for (absolute, relative) in relevant_files {
        if absolute.exists() && absolute.is_file() {
            let result = match ensure_fresh_file(workspace, db_path, &conn, &relative) {
                Ok(_) => Ok(()),
                Err(_) => update_file(workspace, db_path, &relative),
            };
            if let Err(error) = result {
                warn!("Failed to refresh '{}' from watcher: {error}", relative);
                record_failure(
                    failures,
                    format!("Failed to refresh '{relative}' from watcher: {error}"),
                );
            }
        } else if !absolute.exists() {
            remove_indexed_path(workspace, db_path, &conn, &relative, failures);
        }
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
        let is_registered = registrations
            .paths
            .contains_key(&RegistrationPath::new(&absolute));
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

    static ROOT_WATCH_FAULT_TEST_LOCK: Mutex<()> = Mutex::new(());

    #[cfg(target_os = "linux")]
    struct RootWatchFaultReset;

    #[cfg(target_os = "linux")]
    impl Drop for RootWatchFaultReset {
        fn drop(&mut self) {
            FAIL_NEXT_ROOT_WATCH.store(false, Ordering::Release);
        }
    }

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
        registrations.paths.insert(RegistrationPath::new(root), 0);

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
        let root = temp_dir.path().to_path_buf();
        std::fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("ignored")).unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        let workspace = Workspace::new(root.clone());
        let mut registrations = RegisteredDirectories::default();
        registrations.paths.insert(RegistrationPath::new(root), 0);

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
            failures: Arc::clone(&failures),
            shutdown: Arc::new(AtomicBool::new(false)),
            worker: None,
        };
        record_failure(&failures, "index update failed".to_string());
        let generation = handle.pending_failure().unwrap().0;

        assert!(handle.clear_failure_if_unchanged(generation));
        assert!(handle.pending_failure().is_none());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn failed_root_rewatch_is_retried_before_future_root_edits() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _fault_reset = RootWatchFaultReset;
        FAIL_NEXT_ROOT_WATCH.store(false, Ordering::Release);
        ROOT_WATCH_FAILURE_COUNT.store(0, Ordering::Release);
        crate::sync::find_julie_extract_binary()
            .expect("julie-extract binary must be present for tests");

        let temp_dir = crate::safe_tempdir();
        let root = temp_dir.path().join("project");
        let removed_tree = root.join("removed/deep");
        std::fs::create_dir_all(&removed_tree).unwrap();
        std::fs::write(root.join("main.rs"), "pub fn root_watch_seed() {}\n").unwrap();
        std::fs::write(
            removed_tree.join("gone.rs"),
            "pub fn removed_subtree_symbol() {}\n",
        )
        .unwrap();
        let workspace = Workspace::new(root.clone());
        let db_path = temp_dir.path().join("root_watch_retry.db");
        crate::sync::scan_workspace(&workspace, &db_path, true).unwrap();
        let watcher = start_watcher(workspace, db_path.clone()).unwrap();
        wait_for_indexed_symbol(&db_path, "removed_subtree_symbol", true);

        FAIL_NEXT_ROOT_WATCH.store(true, Ordering::Release);
        std::fs::remove_dir_all(root.join("removed")).unwrap();
        wait_until("injected root re-watch failure", || {
            ROOT_WATCH_FAILURE_COUNT.load(Ordering::Acquire) == 1
        });
        wait_until("registration failure is visible", || {
            watcher
                .pending_failure()
                .is_some_and(|(_, failure)| failure.contains("workspace watches"))
        });
        wait_until("root registration recovers", || {
            watcher.pending_failure().is_none()
        });

        let direct_file = root.join("direct.rs");
        std::fs::write(&direct_file, "pub fn after_root_watch_recovery() {}\n").unwrap();
        wait_for_indexed_symbol(&db_path, "after_root_watch_recovery", true);
        std::fs::write(
            &direct_file,
            "pub fn edited_after_root_watch_recovery() {}\n",
        )
        .unwrap();
        wait_for_indexed_symbol(&db_path, "after_root_watch_recovery", false);
        wait_for_indexed_symbol(&db_path, "edited_after_root_watch_recovery", true);

        assert!(watcher.pending_failure().is_none());
        drop(watcher);
    }

    #[test]
    fn registration_rewalks_when_a_discovered_directory_disappears() {
        let _guard = ROOT_WATCH_FAULT_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let temp_dir = crate::safe_tempdir();
        let root = temp_dir.path().join("project");
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

        let result = synchronize_registrations(&mut debouncer, &workspace, &mut registrations);

        assert!(
            result.is_ok(),
            "the registration walk should recover after a discovered directory disappears: {result:?}"
        );
        assert!(!disappearing.exists());
        assert_eq!(registrations.generation, 2);
        assert_eq!(registrations.paths.len(), 1);
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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
