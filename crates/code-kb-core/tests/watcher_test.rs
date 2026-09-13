use code_kb_core::{
    Workspace, find_julie_extract_binary, open_read_only, scan_workspace, search_symbols,
    start_watcher,
};
use std::fs;
use std::thread::sleep;
use std::time::Duration;

#[test]
fn test_background_watcher_incremental_sync() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping watcher test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    let file1 = src_dir.join("greet.rs");
    fs::write(&file1, "pub fn greet() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("watcher_test.db");

    // Initial scan
    scan_workspace(&ws, &db_path, true).expect("Initial scan failed");

    // Start Tier 3 file watcher
    let _watcher = start_watcher(ws.clone(), db_path.clone()).expect("Failed to start watcher");

    // Verify greet exists
    {
        let conn = open_read_only(&db_path).unwrap();
        let syms = search_symbols(&conn, "greet", None, false, 10).unwrap();
        assert_eq!(syms.len(), 1);
        assert_eq!(syms[0].name, "greet");
    }

    // 1. External edit to existing file: add farewell function
    fs::write(&file1, "pub fn greet() {}\npub fn farewell() {}\n").unwrap();

    // Wait for debouncer (150ms) and update execution
    let mut updated = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if let Ok(syms) = search_symbols(&conn, "farewell", None, false, 10)
            && !syms.is_empty()
        {
            updated = true;
            break;
        }
    }
    assert!(
        updated,
        "Watcher failed to detect external modification to greet.rs"
    );

    // 2. External creation of new file
    let file2 = src_dir.join("extra.rs");
    fs::write(&file2, "pub fn bonus_feature() {}\n").unwrap();

    let mut created = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if let Ok(syms) = search_symbols(&conn, "bonus_feature", None, false, 10)
            && !syms.is_empty()
        {
            created = true;
            break;
        }
    }
    assert!(created, "Watcher failed to detect new file extra.rs");

    // 3. External deletion of file
    fs::remove_file(&file2).unwrap();

    let mut deleted = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if let Ok(syms) = search_symbols(&conn, "bonus_feature", None, false, 10)
            && syms.is_empty()
        {
            deleted = true;
            break;
        }
    }
    assert!(deleted, "Watcher failed to detect deletion of extra.rs");
}

#[test]
fn test_watcher_updates_directory_named_targeted() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping watcher test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("targeted");
    fs::create_dir_all(&src_dir).unwrap();
    let file_path = src_dir.join("sample.rs");
    fs::write(&file_path, "pub fn before_update() {}\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("watcher_test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let _watcher = start_watcher(ws, db_path.clone()).unwrap();

    fs::write(&file_path, "pub fn after_update() {}\n").unwrap();

    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if !search_symbols(&conn, "after_update", None, false, 10)
            .unwrap()
            .is_empty()
        {
            return;
        }
    }

    panic!("Watcher did not update a directory named targeted");
}

#[test]
fn test_watcher_respects_nested_ignore_files() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping watcher test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join(".gitignore"), "ignored.rs\n").unwrap();
    fs::write(src_dir.join(".julieignore"), "julie_ignored.rs\n").unwrap();

    let ws = Workspace::new(root.clone());
    let db_path = root.join("watcher_test.db");
    scan_workspace(&ws, &db_path, true).unwrap();
    let _watcher = start_watcher(ws, db_path.clone()).unwrap();

    fs::write(src_dir.join("ignored.rs"), "pub fn ignored_symbol() {}\n").unwrap();
    fs::write(
        src_dir.join("julie_ignored.rs"),
        "pub fn julie_ignored_symbol() {}\n",
    )
    .unwrap();
    fs::write(src_dir.join("tracked.rs"), "pub fn tracked_symbol() {}\n").unwrap();

    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if !search_symbols(&conn, "tracked_symbol", None, false, 10)
            .unwrap()
            .is_empty()
        {
            assert!(
                search_symbols(&conn, "ignored_symbol", None, false, 10)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                search_symbols(&conn, "julie_ignored_symbol", None, false, 10)
                    .unwrap()
                    .is_empty()
            );
            return;
        }
    }

    panic!("Watcher did not index tracked file");
}

#[test]
fn test_watcher_does_not_reindex_on_reads() {
    let extract_bin = find_julie_extract_binary();
    if extract_bin.is_none() {
        eprintln!("Skipping watcher test: julie-extract binary not found");
        return;
    }

    let temp_dir = tempfile::tempdir().unwrap();
    let root = temp_dir.path().to_path_buf();
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();

    let mut paths = Vec::new();
    for i in 0..10 {
        let file_path = src_dir.join(format!("file_{i}.rs"));
        fs::write(&file_path, format!("pub fn func_{i}() {{}}\n")).unwrap();
        paths.push(file_path);
    }

    let ws = Workspace::new(root.clone());
    let db_path = root.join("watcher_reads_test.db");
    scan_workspace(&ws, &db_path, true).unwrap();

    // Collect initial indexed_at timestamps
    let get_timestamps = || -> Vec<(String, String)> {
        let conn = open_read_only(&db_path).unwrap();
        let mut stmt = conn
            .prepare("SELECT path, indexed_at FROM files ORDER BY path ASC")
            .unwrap();
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap();
        rows.map(|r| r.unwrap()).collect()
    };

    let initial_timestamps = get_timestamps();
    assert_eq!(initial_timestamps.len(), 10);

    let _watcher = start_watcher(ws, db_path.clone()).unwrap();

    // Read every file multiple times to simulate inspection/slicing
    for _ in 0..3 {
        for path in &paths {
            let _ = fs::read_to_string(path).unwrap();
        }
    }

    // Sleep for 500ms (longer than 150ms debounce window)
    sleep(Duration::from_millis(500));

    let final_timestamps = get_timestamps();
    assert_eq!(
        initial_timestamps, final_timestamps,
        "Reading files must not trigger watcher re-indexing"
    );
}
