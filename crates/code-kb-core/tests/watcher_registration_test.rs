use code_kb_core::{
    db::open_read_only,
    queries::search_symbols,
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    watcher::start_watcher,
    workspace::Workspace,
};
#[cfg(target_os = "linux")]
use std::io;
use std::{fs, path::Path, sync::Mutex, thread::sleep, time::Duration};

static WATCHER_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn watcher_reconciles_ignore_file_transitions() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    let source = root.join("src");
    fs::create_dir_all(&source).unwrap();
    fs::write(
        source.join("lib.rs"),
        "pub fn ignored_after_rule_change() {}\n",
    )
    .unwrap();
    fs::write(
        source.join("other.rs"),
        "pub fn restored_after_rule_edit() {}\n",
    )
    .unwrap();

    let workspace = Workspace::new(root.to_path_buf());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();
    #[cfg(target_os = "linux")]
    let initial_watch_count = inotify_watch_count().unwrap();

    wait_for_symbol(&db_path, "ignored_after_rule_change", true);
    wait_for_symbol(&db_path, "restored_after_rule_edit", true);
    fs::write(root.join(".gitignore"), "src/\n").unwrap();
    wait_for_symbol(&db_path, "ignored_after_rule_change", false);
    wait_for_symbol(&db_path, "restored_after_rule_edit", false);
    #[cfg(target_os = "linux")]
    assert!(inotify_watch_count().unwrap() < initial_watch_count);

    fs::write(root.join(".gitignore"), "src/lib.rs\n").unwrap();
    wait_for_symbol(&db_path, "restored_after_rule_edit", true);
    wait_for_symbol(&db_path, "ignored_after_rule_change", false);
    #[cfg(target_os = "linux")]
    assert_eq!(inotify_watch_count().unwrap(), initial_watch_count);

    fs::remove_file(root.join(".gitignore")).unwrap();
    wait_for_symbol(&db_path, "ignored_after_rule_change", true);
    wait_for_symbol(&db_path, "restored_after_rule_edit", true);
    #[cfg(target_os = "linux")]
    assert_eq!(inotify_watch_count().unwrap(), initial_watch_count);
    assert!(
        watcher.pending_failure().is_none(),
        "watcher reported a failure: {:?}",
        watcher.pending_failure()
    );

    drop(watcher);
}

#[test]
fn watcher_reconciles_edits_made_before_startup_registration() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let file = root.join("src/deep/lib.rs");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "pub fn before_startup_registration() {}\n").unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    fs::write(&file, "pub fn changed_before_startup_registration() {}\n").unwrap();
    let conn = open_read_only(&db_path).unwrap();
    assert!(
        !search_symbols(
            &conn,
            "changed_before_startup_registration",
            None,
            false,
            10
        )
        .unwrap()
        .iter()
        .any(|symbol| symbol.name == "changed_before_startup_registration")
    );
    drop(conn);

    let watcher = start_watcher(workspace, db_path.clone()).unwrap();
    wait_for_symbol(&db_path, "changed_before_startup_registration", true);
    assert!(watcher.pending_failure().is_none());

    drop(watcher);
}

#[test]
fn watcher_reconciles_new_moved_deleted_and_recreated_subtrees() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.rs"), "pub fn initial_symbol() {}\n").unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();
    #[cfg(target_os = "linux")]
    let initial_watch_count = inotify_watch_count().unwrap();

    let nested = root.join("new/deep/tree");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        nested.join("created.rs"),
        "pub fn immediate_nested_symbol() {}\n",
    )
    .unwrap();
    wait_for_symbol(&db_path, "immediate_nested_symbol", true);

    let incoming = temp_dir.path().join("incoming");
    let incoming_nested = incoming.join("deep");
    fs::create_dir_all(&incoming_nested).unwrap();
    fs::write(
        incoming_nested.join("moved.rs"),
        "pub fn populated_move_symbol() {}\n",
    )
    .unwrap();
    let moved = root.join("moved");
    fs::rename(&incoming, &moved).unwrap();
    wait_for_symbol(&db_path, "populated_move_symbol", true);

    fs::remove_dir_all(&moved).unwrap();
    wait_for_symbol(&db_path, "populated_move_symbol", false);
    #[cfg(target_os = "linux")]
    wait_for_watch_count(initial_watch_count + 3);

    let recreated = moved.join("deep");
    fs::create_dir_all(&recreated).unwrap();
    let recreated_file = recreated.join("recreated.rs");
    fs::write(&recreated_file, "pub fn recreated_subtree_symbol() {}\n").unwrap();
    wait_for_symbol(&db_path, "recreated_subtree_symbol", true);
    fs::write(
        &recreated_file,
        "pub fn edited_recreated_subtree_symbol() {}\n",
    )
    .unwrap();
    wait_for_symbol(&db_path, "recreated_subtree_symbol", false);
    wait_for_symbol(&db_path, "edited_recreated_subtree_symbol", true);
    #[cfg(target_os = "linux")]
    wait_for_watch_count(initial_watch_count + 5);
    assert!(
        watcher.pending_failure().is_none(),
        "watcher reported a failure: {:?}",
        watcher.pending_failure()
    );

    drop(watcher);
}

#[test]
fn watcher_indexes_atomic_file_replacements_incrementally() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    let source = root.join("atomic.rs");
    fs::write(&source, "pub fn before_atomic_replace() {}\n").unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();
    let replacement = root.join("atomic.rs.tmp");
    fs::write(&replacement, "pub fn after_atomic_replace() {}\n").unwrap();
    #[cfg(not(windows))]
    fs::rename(&replacement, &source).unwrap();
    #[cfg(windows)]
    {
        let backup = root.join("atomic.rs.old");
        fs::rename(&source, &backup).unwrap();
        fs::rename(&replacement, &source).unwrap();
        fs::remove_file(backup).unwrap();
    }

    wait_for_symbol(&db_path, "before_atomic_replace", false);
    wait_for_symbol(&db_path, "after_atomic_replace", true);
    assert!(watcher.pending_failure().is_none());

    drop(watcher);
}

#[cfg(unix)]
#[test]
fn watcher_retries_failed_directory_registration_and_watches_after_recovery() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("main.rs"),
        "pub fn registration_failure_seed() {}\n",
    )
    .unwrap();
    let workspace = Workspace::new(root.clone());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();

    let blocked = root.join("blocked");
    fs::create_dir(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0)).unwrap();
    let (generation, failure) = wait_for_registration_failure(&watcher);
    assert!(failure.contains("workspace watches"));
    assert!(!watcher.clear_failure_if_unchanged(generation));
    assert!(watcher.pending_failure().is_some());

    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o700)).unwrap();
    let nested = blocked.join("deep");
    fs::create_dir_all(&nested).unwrap();
    let source = nested.join("recover.rs");
    fs::write(&source, "pub fn indexed_after_registration_recovery() {}\n").unwrap();
    wait_for_symbol(&db_path, "indexed_after_registration_recovery", true);
    wait_for_failure_message_to_change(&watcher, &failure);

    fs::write(&source, "pub fn changed_after_registration_recovery() {}\n").unwrap();
    wait_for_symbol(&db_path, "indexed_after_registration_recovery", false);
    wait_for_symbol(&db_path, "changed_after_registration_recovery", true);

    drop(watcher);
}

#[test]
fn watcher_recovers_a_large_file_event_storm() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.rs"), "pub fn initial_storm_symbol() {}\n").unwrap();

    let workspace = Workspace::new(root.clone());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();

    for index in 0..75 {
        fs::write(
            root.join(format!("storm_{index:03}.rs")),
            format!("pub fn watcher_storm_{index:03}() {{}}\n"),
        )
        .unwrap();
    }

    for _ in 0..50 {
        let conn = open_read_only(&db_path).unwrap();
        let symbols = search_symbols(&conn, "watcher_storm", None, false, 100).unwrap();
        if symbols.len() == 75 {
            assert!(watcher.pending_failure().is_none());
            drop(watcher);
            return;
        }
        sleep(Duration::from_millis(100));
    }
    panic!("watcher did not reconcile every file from the event storm");
}

fn wait_for_symbol(db_path: &Path, name: &str, expected: bool) {
    for _ in 0..50 {
        let conn = open_read_only(db_path).unwrap();
        let found = search_symbols(&conn, name, None, false, 10)
            .unwrap()
            .iter()
            .any(|symbol| symbol.name == name);
        if found == expected {
            return;
        }
        sleep(Duration::from_millis(100));
    }
    panic!("symbol '{name}' did not reach indexed state {expected}");
}

fn wait_for_registration_failure(watcher: &code_kb_core::watcher::WatcherHandle) -> (u64, String) {
    for _ in 0..50 {
        if let Some((generation, failure)) = watcher.pending_failure()
            && failure.contains("workspace watches")
        {
            return (generation, failure);
        }
        sleep(Duration::from_millis(100));
    }
    panic!("watcher did not report the directory registration failure");
}

fn wait_for_failure_message_to_change(
    watcher: &code_kb_core::watcher::WatcherHandle,
    previous: &str,
) {
    for _ in 0..50 {
        if watcher
            .pending_failure()
            .is_none_or(|(_, failure)| failure != previous)
        {
            return;
        }
        sleep(Duration::from_millis(100));
    }
    panic!("watcher retained the directory registration failure after recovery");
}

#[cfg(target_os = "linux")]
fn wait_for_watch_count(expected: usize) {
    let mut actual = 0;
    for _ in 0..50 {
        actual = inotify_watch_count().unwrap();
        if actual == expected {
            return;
        }
        sleep(Duration::from_millis(100));
    }
    panic!("inotify watch count did not reach {expected}; last count was {actual}");
}

#[cfg(target_os = "linux")]
fn wait_for_inotify_instance_count(expected: usize) {
    for _ in 0..50 {
        if inotify_instance_count().unwrap() == expected {
            return;
        }
        sleep(Duration::from_millis(100));
    }
    assert_eq!(inotify_instance_count().unwrap(), expected);
}

#[cfg(target_os = "linux")]
#[test]
fn watcher_startup_failure_releases_notify_backend() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("missing");
    let before = inotify_instance_count().unwrap();

    assert!(start_watcher(Workspace::new(root.clone()), root.join("index.db")).is_err());

    wait_for_inotify_instance_count(before);
}

#[cfg(target_os = "linux")]
#[test]
fn watcher_startup_reconcile_failure_releases_notify_backend() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("main.rs"), "pub fn startup_scan_failure() {}\n").unwrap();
    let db_path = temp_dir.path().join("index-is-a-directory");
    fs::create_dir(&db_path).unwrap();
    let before = inotify_instance_count().unwrap();

    assert!(start_watcher(Workspace::new(root), db_path).is_err());

    assert_eq!(inotify_instance_count().unwrap(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn watcher_does_not_register_ignored_descendants() {
    let _guard = WATCHER_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().join("project");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join(".gitignore"), "vendor/\nlate-ignore/\n").unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/lib.rs"), "pub fn allowed() {}\n").unwrap();

    for branch in 0..8 {
        let mut directory = root.join("vendor").join(format!("branch_{branch}"));
        for depth in 0..8 {
            directory.push(format!("nested_{depth}"));
            fs::create_dir_all(&directory).unwrap();
        }
    }

    let workspace = Workspace::new(root.to_path_buf());
    let db_path = temp_dir.path().join("watcher_registration.db");
    scan_workspace(&workspace, &db_path, true).unwrap();

    let before = inotify_watch_count().unwrap();
    let watcher = start_watcher(workspace, db_path.clone()).unwrap();
    let after = inotify_watch_count().unwrap();
    let added = after.saturating_sub(before);

    assert!(
        added <= 3,
        "watcher registered {added} directory watches despite the ignored vendor tree"
    );

    let late_ignored = root.join("late-ignore");
    for branch in 0..8 {
        let mut directory = late_ignored.join(format!("branch_{branch}"));
        for depth in 0..8 {
            directory.push(format!("nested_{depth}"));
            fs::create_dir_all(&directory).unwrap();
        }
    }
    fs::write(
        root.join("src/trigger.rs"),
        "pub fn allowed_sibling_trigger() {}\n",
    )
    .unwrap();
    wait_for_symbol(&db_path, "allowed_sibling_trigger", true);
    assert_eq!(
        inotify_watch_count().unwrap(),
        after,
        "newly created ignored descendants received directory watches"
    );

    drop(watcher);
}

#[cfg(target_os = "linux")]
fn inotify_watch_count() -> io::Result<usize> {
    let fdinfo = Path::new("/proc/self/fdinfo");
    let mut count = 0;
    for entry in fs::read_dir(fdinfo)? {
        let entry = entry?;
        let contents = match fs::read_to_string(entry.path()) {
            Ok(contents) => contents,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        };
        count += contents
            .lines()
            .filter(|line| line.starts_with("inotify wd:"))
            .count();
    }
    Ok(count)
}

#[cfg(target_os = "linux")]
fn inotify_instance_count() -> io::Result<usize> {
    let mut count = 0;
    for entry in fs::read_dir("/proc/self/fd")? {
        match fs::read_link(entry?.path()) {
            Ok(path) if path.to_string_lossy() == "anon_inode:inotify" => count += 1,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(count)
}
