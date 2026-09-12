use std::fs;
use std::thread::sleep;
use std::time::Duration;
use code_kb_core::{
    find_julie_extract_binary, open_read_only, scan_workspace, search_symbols,
    start_watcher, Workspace,
};

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
        if let Ok(syms) = search_symbols(&conn, "farewell", None, false, 10) {
            if !syms.is_empty() {
                updated = true;
                break;
            }
        }
    }
    assert!(updated, "Watcher failed to detect external modification to greet.rs");

    // 2. External creation of new file
    let file2 = src_dir.join("extra.rs");
    fs::write(&file2, "pub fn bonus_feature() {}\n").unwrap();

    let mut created = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if let Ok(syms) = search_symbols(&conn, "bonus_feature", None, false, 10) {
            if !syms.is_empty() {
                created = true;
                break;
            }
        }
    }
    assert!(created, "Watcher failed to detect new file extra.rs");

    // 3. External deletion of file
    fs::remove_file(&file2).unwrap();

    let mut deleted = false;
    for _ in 0..25 {
        sleep(Duration::from_millis(200));
        let conn = open_read_only(&db_path).unwrap();
        if let Ok(syms) = search_symbols(&conn, "bonus_feature", None, false, 10) {
            if syms.is_empty() {
                deleted = true;
                break;
            }
        }
    }
    assert!(deleted, "Watcher failed to detect deletion of extra.rs");
}
