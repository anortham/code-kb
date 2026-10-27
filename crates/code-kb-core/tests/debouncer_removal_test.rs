use std::{
    path::Path,
    sync::{
        Mutex, OnceLock,
        mpsc::{self, Sender},
    },
    time::Duration,
};

use code_kb_core::{
    db::open_read_only,
    queries::search_symbols,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use notify_debouncer_full::{
    NoCache, new_debouncer_opt,
    notify::{
        Config, Event, EventHandler, EventKind, RecursiveMode, Watcher, WatcherKind,
        event::{CreateKind, RemoveKind},
    },
};

fn raw_event_sender() -> &'static Mutex<Option<Sender<notify_debouncer_full::notify::Result<Event>>>>
{
    static RAW_EVENT_SENDER: OnceLock<
        Mutex<Option<Sender<notify_debouncer_full::notify::Result<Event>>>>,
    > = OnceLock::new();
    RAW_EVENT_SENDER.get_or_init(|| Mutex::new(None))
}

struct SyntheticWatcher;

impl Watcher for SyntheticWatcher {
    fn new<F: EventHandler>(
        mut event_handler: F,
        _config: Config,
    ) -> notify_debouncer_full::notify::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            for event in receiver {
                event_handler.handle_event(event);
            }
        });
        *raw_event_sender().lock().unwrap() = Some(sender);
        Ok(Self)
    }

    fn watch(
        &mut self,
        _path: &Path,
        _recursive_mode: RecursiveMode,
    ) -> notify_debouncer_full::notify::Result<()> {
        Ok(())
    }

    fn unwatch(&mut self, _path: &Path) -> notify_debouncer_full::notify::Result<()> {
        Ok(())
    }

    fn configure(&mut self, _config: Config) -> notify_debouncer_full::notify::Result<bool> {
        Ok(true)
    }

    fn kind() -> WatcherKind {
        notify_debouncer_full::notify::RecommendedWatcher::kind()
    }
}

struct SyntheticEventReset;

impl Drop for SyntheticEventReset {
    fn drop(&mut self) {
        raw_event_sender().lock().unwrap().take();
    }
}

fn emit(event: Event) {
    raw_event_sender()
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .send(Ok(event))
        .unwrap();
}

#[test]
fn debouncer_emits_remove_after_create_for_an_indexed_path() {
    let directory = tempfile::tempdir().unwrap();
    let indexed_path = directory.path().join("moved.rs");
    std::fs::write(&indexed_path, "pub fn previously_visible() {}\n").unwrap();
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let workspace = Workspace::new(directory.path().to_path_buf());
    let db_path = directory.path().join("watcher_test.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    assert!(
        !search_symbols(&conn, "previously_visible", None, false, 10)
            .unwrap()
            .is_empty()
    );
    drop(conn);
    std::fs::remove_file(&indexed_path).unwrap();

    let _event_reset = SyntheticEventReset;
    let (result_sender, result_receiver) = mpsc::channel();
    let mut debouncer = new_debouncer_opt::<_, SyntheticWatcher, NoCache>(
        Duration::from_millis(300),
        Some(Duration::from_millis(100)),
        result_sender,
        NoCache,
        Config::default(),
    )
    .unwrap();
    debouncer
        .watch(directory.path(), RecursiveMode::Recursive)
        .unwrap();

    emit(Event::new(EventKind::Create(CreateKind::Any)).add_path(indexed_path.clone()));
    emit(Event::new(EventKind::Remove(RemoveKind::Any)).add_path(indexed_path.clone()));

    let events = result_receiver
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    assert!(events.iter().any(|event| {
        matches!(event.event.kind, EventKind::Remove(RemoveKind::Any))
            && event.event.paths == vec![indexed_path.clone()]
    }));
}
