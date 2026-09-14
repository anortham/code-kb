use serde_json::{Value, json};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

struct ChildGuard(Child);

impl std::ops::Deref for ChildGuard {
    type Target = Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn create_full_schema(conn: &rusqlite::Connection) {
    conn.execute_batch(
        "CREATE TABLE files (
            file_id TEXT PRIMARY KEY, path TEXT, language TEXT, content_hash TEXT,
            content_bytes INTEGER, line_count INTEGER, indexed_at TEXT
        );
        CREATE TABLE symbols (
            symbol_id TEXT PRIMARY KEY, file_id TEXT, path TEXT, language TEXT, name TEXT, kind TEXT,
            signature TEXT, doc_comment TEXT, visibility TEXT, parent_symbol_id TEXT,
            start_line INTEGER, start_column INTEGER, end_line INTEGER, end_column INTEGER,
            start_byte INTEGER, end_byte INTEGER, body_start_line INTEGER,
            body_start_column INTEGER, body_end_line INTEGER, body_end_column INTEGER,
            body_start_byte INTEGER, body_end_byte INTEGER, body_hash TEXT,
            semantic_group TEXT, is_test INTEGER, test_container INTEGER
        );
        CREATE TABLE relationships (
            relationship_id TEXT PRIMARY KEY, from_symbol_id TEXT, to_symbol_id TEXT,
            kind TEXT, path TEXT, start_line INTEGER, start_column INTEGER
        );
        CREATE TABLE pending_relationships (
            from_symbol_id TEXT, target_terminal_name TEXT, kind TEXT,
            path TEXT, start_line INTEGER, start_column INTEGER
        );",
    )
    .unwrap();
    code_kb_core::db::ensure_fts_index(conn).unwrap();
}

// ============================================================================
// TARGET 1: Worktree Fast-Path WAL Flush Before Copy in server.rs
// ============================================================================

#[test]
fn test_adversarial_worktree_fastpath_wal_flush_uncheckpointed_transactions() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-wal");

    fs::create_dir_all(main_root.join(".git")).unwrap();
    fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_code = "pub fn base_function() {}\n";
    fs::write(&main_file, main_code).unwrap();

    let main_db = main_root.join(".code-kb").join("artifact.db");
    let wal_path = main_root.join(".code-kb").join("artifact.db-wal");

    // Initialize DB with schema in WAL mode
    let conn = code_kb_core::open_read_write(&main_db).unwrap();
    create_full_schema(&conn);

    // Disable auto-checkpointing on this connection so all new commits stay in WAL
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .unwrap();

    let hash_base = format!("blake3:{}", blake3::hash(main_code.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f0', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![hash_base, main_code.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's0', 'f0', 'src/main.rs', 'rust', 'base_function', 'function',
            'pub fn base_function()', NULL, 'pub', NULL,
            1, 0, 1, 23, 0, 23, 1, 0, 1, 23, 0, 23, 'b3:hash',
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();

    // Now insert 200 additional symbols in separate transactions without checkpointing
    for i in 1..=200 {
        let sym_name = format!("wal_committed_fn_{i}");
        let file_path = format!("src/module_{i}.rs");
        let fid = format!("f{i}");
        let sid = format!("s{i}");
        let content = format!("pub fn {sym_name}() {{}}\n");
        let h = format!("blake3:{}", blake3::hash(content.as_bytes()).to_hex());

        conn.execute(
            "INSERT INTO files VALUES (?1, ?2, 'rust', ?3, ?4, 1, '2026-01-01')",
            rusqlite::params![fid, file_path, h, content.len() as i64],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO symbols VALUES (
                ?1, ?2, ?3, 'rust', ?4, 'function',
                ?5, NULL, 'pub', NULL,
                1, 0, 1, 30, 0, 30, 1, 0, 1, 30, 0, 30, 'b3:hash',
                NULL, 0, 0
            )",
            rusqlite::params![
                sid,
                fid,
                file_path,
                sym_name,
                format!("pub fn {sym_name}()")
            ],
        )
        .unwrap();
    }

    // Keep conn OPEN to simulate the parent agent/server holding the DB open with uncheckpointed WAL
    assert!(
        wal_path.exists(),
        "artifact.db-wal must exist with committed transactions"
    );
    let wal_len_before = fs::metadata(&wal_path).unwrap().len();
    assert!(
        wal_len_before > 0,
        "artifact.db-wal must have non-zero length before worktree copy: got {wal_len_before}"
    );

    // Set up worktree pointing to main repo gitdir
    fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root.join(".git").join("worktrees").join("feature-wal");
    fs::create_dir_all(&gitdir_path).unwrap();
    fs::write(
        wt_root.join(".git"),
        format!("gitdir: {}\n", gitdir_path.display()),
    )
    .unwrap();

    // Copy main.rs into worktree
    let wt_file = wt_root.join("src").join("main.rs");
    fs::write(&wt_file, main_code).unwrap();

    let wt_db = wt_root.join(".code-kb").join("artifact.db");
    assert!(
        !wt_db.exists(),
        "Worktree DB must NOT exist before fast-path"
    );

    // Spawn code-kb serve pointing to main_root
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // 1. Initialize handshake
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "challenger-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // 2. Query file_skeleton on worktree file -> triggers worktree auto-copy fast path
    // In server.rs, it runs:
    // open_read_write(&parent_db) -> checkpoint_truncate(&parent_conn) -> copy(&parent_db, &self.db_path)
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line2 = serde_json::to_string(&wt_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).expect("Invalid JSON response");
    assert_eq!(resp2["id"], 2);
    let body_text = resp2["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        body_text.contains("base_function"),
        "Fast path skeleton must contain base_function"
    );

    // 3. Verify worktree DB was created
    assert!(
        wt_db.exists(),
        "Worktree DB must exist on disk after fast path"
    );

    // 4. Verify parent WAL was truncated to 0 bytes by checkpoint_truncate
    let wal_len_after = fs::metadata(&wal_path).unwrap().len();
    assert_eq!(
        wal_len_after, 0,
        "Parent WAL must be truncated to 0 bytes after fast-path checkpoint"
    );

    // 5. Open worktree DB directly and EMPIRICALLY VERIFY:
    // a) Integrity check passes
    // b) ALL 201 symbols (base + 200 WAL symbols) are present in the copied DB!
    {
        let wt_conn =
            code_kb_core::open_read_only(&wt_db).expect("Failed to open copied worktree DB");
        let integrity: String = wt_conn
            .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            integrity, "ok",
            "Copied worktree DB must pass integrity check without corruption"
        );

        let sym_count: i64 = wt_conn
            .query_row("SELECT count(*) FROM symbols;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            sym_count, 201,
            "Copied worktree DB must contain all 201 symbols flushed from parent WAL"
        );

        // Verify the 200th WAL symbol specifically exists in the worktree DB
        let check_sym: String = wt_conn
            .query_row(
                "SELECT name FROM symbols WHERE name = 'wal_committed_fn_200';",
                [],
                |r| r.get(0),
            )
            .expect("wal_committed_fn_200 must exist in copied worktree DB");
        assert_eq!(check_sym, "wal_committed_fn_200");
    }

    drop(conn);
    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_adversarial_worktree_fastpath_with_active_parent_reader() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-reader");

    fs::create_dir_all(main_root.join(".git")).unwrap();
    fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_code = "pub fn reader_test_fn() {}\n";
    fs::write(&main_file, main_code).unwrap();

    let main_db = main_root.join(".code-kb").join("artifact.db");
    let conn = code_kb_core::open_read_write(&main_db).unwrap();
    create_full_schema(&conn);

    let h = format!("blake3:{}", blake3::hash(main_code.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![h, main_code.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/main.rs', 'rust', 'reader_test_fn', 'function',
            'pub fn reader_test_fn()', NULL, 'pub', NULL,
            1, 0, 1, 26, 0, 26, 1, 0, 1, 26, 0, 26, 'b3:hash',
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();
    drop(conn);

    // Keep an active read-only connection OPEN on parent DB
    let active_reader = code_kb_core::open_read_only(&main_db).expect("Open active reader");

    // Set up worktree
    fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root
        .join(".git")
        .join("worktrees")
        .join("feature-reader");
    fs::create_dir_all(&gitdir_path).unwrap();
    fs::write(
        wt_root.join(".git"),
        format!("gitdir: {}\n", gitdir_path.display()),
    )
    .unwrap();
    let wt_file = wt_root.join("src").join("main.rs");
    fs::write(&wt_file, main_code).unwrap();

    let wt_db = wt_root.join(".code-kb").join("artifact.db");

    // Spawn server
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "challenger-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // Query on worktree file while active reader is still open
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line2 = serde_json::to_string(&wt_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(
        resp2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("reader_test_fn")
    );

    // Verify worktree DB copied cleanly and integrity is ok
    assert!(wt_db.exists(), "Worktree DB must exist");
    let wt_conn = code_kb_core::open_read_only(&wt_db).unwrap();
    let integrity: String = wt_conn
        .query_row("PRAGMA integrity_check;", [], |r| r.get(0))
        .unwrap();
    assert_eq!(integrity, "ok");

    drop(active_reader);
    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_adversarial_worktree_relative_gitdir_resolution() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-rel");

    fs::create_dir_all(main_root.join(".git")).unwrap();
    fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_code = "pub fn rel_gitdir_fn() {}\n";
    fs::write(&main_file, main_code).unwrap();

    let main_db = main_root.join(".code-kb").join("artifact.db");
    let conn = code_kb_core::open_read_write(&main_db).unwrap();
    create_full_schema(&conn);

    let h = format!("blake3:{}", blake3::hash(main_code.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![h, main_code.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/main.rs', 'rust', 'rel_gitdir_fn', 'function',
            'pub fn rel_gitdir_fn()', NULL, 'pub', NULL,
            1, 0, 1, 25, 0, 25, 1, 0, 1, 25, 0, 25, 'b3:hash',
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();
    drop(conn);

    // Set up worktree with RELATIVE gitdir path
    fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root.join(".git").join("worktrees").join("feature-rel");
    fs::create_dir_all(&gitdir_path).unwrap();

    // Use relative path from wt_root: ../../.git/worktrees/feature-rel
    fs::write(
        wt_root.join(".git"),
        "gitdir: ../../.git/worktrees/feature-rel\n",
    )
    .unwrap();

    let wt_file = wt_root.join("src").join("main.rs");
    fs::write(&wt_file, main_code).unwrap();

    let wt_db = wt_root.join(".code-kb").join("artifact.db");

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "challenger-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // Query on worktree file with relative gitdir
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line2 = serde_json::to_string(&wt_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);
    assert!(
        resp2["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("rel_gitdir_fn"),
        "Worktree with relative gitdir should resolve parent DB cleanly"
    );

    assert!(wt_db.exists(), "Worktree DB must be created via fast path");

    drop(stdin);
    let _ = child.wait();
}

// ============================================================================
// TARGET 2: Watcher Path Handling & LIKE Escaping
// ============================================================================

#[test]
fn test_adversarial_watcher_drive_root_slash_trimming() {
    // Test the exact logic in watcher.rs:75-80:
    // let rel_str = to_forward_slash(rel);
    // let rel_str = rel_str.trim_start_matches('/').to_string();

    let cases = vec![
        // Standard path with leading slash from drive root prefix stripping
        ("/src/main.rs", "src/main.rs"),
        ("//src/main.rs", "src/main.rs"),
        ("///deep/nested/path/mod.rs", "deep/nested/path/mod.rs"),
        // Windows backslash variants converted via to_forward_slash
        (r"\src\components\button.rs", "src/components/button.rs"),
        (r"\\src\components\button.rs", "src/components/button.rs"),
        // Already clean relative paths
        ("src/main.rs", "src/main.rs"),
        // Root itself
        ("/", ""),
        ("///", ""),
        (r"\", ""),
    ];

    for (input, expected) in cases {
        let path = Path::new(input);
        let rel_str = code_kb_core::to_forward_slash(path);
        let trimmed = rel_str.trim_start_matches('/').to_string();
        assert_eq!(
            trimmed, expected,
            "Input {input:?} must normalize to {expected:?}"
        );
        assert!(
            !trimmed.starts_with('/'),
            "Normalized path must NEVER have leading slash: {trimmed}"
        );
        assert!(
            !trimmed.starts_with('\\'),
            "Normalized path must NEVER have leading backslash: {trimmed}"
        );
    }
}

#[test]
fn test_adversarial_watcher_drive_root_strip_prefix_lossy() {
    #[cfg(windows)]
    {
        let drive_root = Path::new("C:\\");

        // Various file paths that could be reported by the notify crate
        let file1 = Path::new("C:\\src\\lib.rs");
        let rel1 = code_kb_core::strip_prefix_lossy(file1, drive_root).unwrap();
        let s1 = code_kb_core::to_forward_slash(rel1);
        let clean1 = s1.trim_start_matches('/').to_string();
        assert_eq!(clean1, "src/lib.rs");

        // Lowercase drive letter: c:\src\lib.rs
        let file2 = Path::new("c:\\src\\lib.rs");
        let rel2 = code_kb_core::strip_prefix_lossy(file2, drive_root).unwrap();
        let s2 = code_kb_core::to_forward_slash(rel2);
        let clean2 = s2.trim_start_matches('/').to_string();
        assert_eq!(clean2, "src/lib.rs");

        // Forward slashes: C:/src/lib.rs
        let file3 = Path::new("C:/src/lib.rs");
        let rel3 = code_kb_core::strip_prefix_lossy(file3, drive_root).unwrap();
        let s3 = code_kb_core::to_forward_slash(rel3);
        let clean3 = s3.trim_start_matches('/').to_string();
        assert_eq!(clean3, "src/lib.rs");

        // Event for the root itself: C:\
        let rel_root = code_kb_core::strip_prefix_lossy(drive_root, drive_root).unwrap();
        let s_root = code_kb_core::to_forward_slash(rel_root);
        let clean_root = s_root.trim_start_matches('/').to_string();
        assert!(
            clean_root.is_empty(),
            "Drive root relative to itself must trim to empty string"
        );
    }
}

#[test]
fn test_adversarial_watcher_directory_deletion_like_escaping_matrix() {
    let temp_dir = code_kb_core::safe_tempdir();
    let db_path = temp_dir.path().join("watcher_like.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();

    conn.execute_batch("CREATE TABLE files (path TEXT PRIMARY KEY);")
        .unwrap();

    // Populate database with files across distinct directories including edge-case characters
    let files = vec![
        // Standard hierarchy
        "src/utils/math.rs",
        "src/utils/string.rs",
        "src/utils/nested/deep.rs",
        "src/utils_extra/other.rs", // Suffix collision: 'src/utils' must NOT match 'src/utils_extra'
        // Wildcard '_' in directory name
        "src/ui_button/render.rs",
        "src/uiXbutton/render.rs", // '_' wildcard collision: 'src/ui_button' must NOT match 'src/uiXbutton'
        // Wildcard '%' in directory name
        "src/progress_100%/summary.rs",
        "src/progress_1000/summary.rs", // '%' wildcard collision: 'src/progress_100%' must NOT match 'src/progress_1000'
        // Brackets in directory name
        "src/brackets[abc]/mod.rs",
        "src/brackets_abc/mod.rs",
        // Unicode directory name
        "src/папка_модулей/файл.rs",
        // Space in directory name
        "src/space dir/test.rs",
        // Deeply nested hierarchy (25 levels)
        "d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/deep_leaf.rs",
    ];

    for f in &files {
        conn.execute("INSERT INTO files VALUES (?1);", [f]).unwrap();
    }

    // Helper: executes the exact watcher.rs query pattern
    let query_children = |dir_rel: &str| -> Vec<String> {
        let escaped = dir_rel
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}/%");
        let mut stmt = conn
            .prepare("SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\'")
            .unwrap();
        let rows = stmt
            .query_map([&pattern], |r| r.get::<_, String>(0))
            .unwrap();
        rows.flatten().collect()
    };

    // 1. Test standard directory deletion
    let utils_children = query_children("src/utils");
    assert_eq!(
        utils_children.len(),
        3,
        "src/utils should match exactly 3 files, got: {utils_children:?}"
    );
    assert!(utils_children.contains(&"src/utils/math.rs".to_string()));
    assert!(utils_children.contains(&"src/utils/string.rs".to_string()));
    assert!(utils_children.contains(&"src/utils/nested/deep.rs".to_string()));
    assert!(
        !utils_children.contains(&"src/utils_extra/other.rs".to_string()),
        "src/utils must NOT match src/utils_extra"
    );

    // 2. Test underscore escaping in directory deletion
    let ui_children = query_children("src/ui_button");
    assert_eq!(
        ui_children,
        vec!["src/ui_button/render.rs"],
        "src/ui_button must only match literal underscore, got: {ui_children:?}"
    );
    assert!(
        !ui_children.contains(&"src/uiXbutton/render.rs".to_string()),
        "src/ui_button must NOT match src/uiXbutton via unescaped '_' wildcard"
    );

    // 3. Test percent escaping in directory deletion
    let pct_children = query_children("src/progress_100%");
    assert_eq!(
        pct_children,
        vec!["src/progress_100%/summary.rs"],
        "src/progress_100% must only match literal '%', got: {pct_children:?}"
    );
    assert!(
        !pct_children.contains(&"src/progress_1000/summary.rs".to_string()),
        "src/progress_100% must NOT match src/progress_1000 via unescaped '%' wildcard"
    );

    // 4. Test brackets in directory name
    let bracket_children = query_children("src/brackets[abc]");
    assert_eq!(bracket_children, vec!["src/brackets[abc]/mod.rs"]);

    // 5. Test unicode directory name
    let unicode_children = query_children("src/папка_модулей");
    assert_eq!(unicode_children, vec!["src/папка_модулей/файл.rs"]);

    // 6. Test space in directory name
    let space_children = query_children("src/space dir");
    assert_eq!(space_children, vec!["src/space dir/test.rs"]);

    // 7. Test deeply nested hierarchy
    let d1_children = query_children("d1");
    assert_eq!(
        d1_children,
        vec!["d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/deep_leaf.rs"]
    );
    let d10_children = query_children("d1/d2/d3/d4/d5/d6/d7/d8/d9/d10");
    assert_eq!(
        d10_children,
        vec!["d1/d2/d3/d4/d5/d6/d7/d8/d9/d10/d11/d12/d13/d14/d15/d16/d17/d18/d19/d20/deep_leaf.rs"]
    );

    // 8. Test threshold trigger (> 50 files)
    for i in 0..55 {
        conn.execute(
            "INSERT INTO files VALUES (?1);",
            [format!("large_batch/file_{i}.rs")],
        )
        .unwrap();
    }
    let large_children = query_children("large_batch");
    assert_eq!(large_children.len(), 55);
    assert!(
        large_children.len() > 50,
        "Circuit-breaker condition (> 50) must be satisfied for large directory deletion"
    );
}

// ============================================================================
// TARGET 3: Core Invariant #1 - MCP Tool Schema Invariant
// ============================================================================

#[test]
fn test_adversarial_core_invariant_1_mcp_tool_schemas_strictly_zero_workspace_parameters() {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    let db_dir = root.join(".code-kb");
    fs::create_dir_all(&db_dir).unwrap();
    let db_path = db_dir.join("artifact.db");

    let conn = code_kb_core::open_read_write(&db_path).unwrap();
    create_full_schema(&conn);
    drop(conn);

    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "challenger-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // Send tools/list
    let list_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    });
    let mut line2 = serde_json::to_string(&list_req).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();

    let resp2: Value = serde_json::from_str(&resp_line2).expect("Failed to parse JSON response");
    assert_eq!(resp2["id"], 2);

    let tools = resp2["result"]["tools"]
        .as_array()
        .expect("Expected tools array in tools/list response");

    assert_eq!(
        tools.len(),
        10,
        "Server must advertise exactly 10 MCP tools"
    );

    // Exhaustive list of forbidden workspace-polluting parameters
    let forbidden_keys = [
        "workspace",
        "workspace_id",
        "repo_path",
        "root_dir",
        "root",
        "cwd",
        "workdir",
        "workspace_path",
        "repository",
        "repository_path",
    ];

    for tool in tools {
        let tool_name = tool["name"].as_str().unwrap();
        let schema = &tool["inputSchema"];
        let props = &schema["properties"];

        // 1. Check properties
        if let Some(props_obj) = props.as_object() {
            for key in props_obj.keys() {
                for forbidden in &forbidden_keys {
                    assert_ne!(
                        key.as_str(),
                        *forbidden,
                        "CRITICAL VIOLATION: Tool '{tool_name}' exposes forbidden parameter '{key}' in inputSchema.properties!"
                    );
                }
            }
        }

        // 2. Check required list
        if let Some(required_arr) = schema["required"].as_array() {
            for req in required_arr {
                let req_str = req.as_str().unwrap();
                for forbidden in &forbidden_keys {
                    assert_ne!(
                        req_str, *forbidden,
                        "CRITICAL VIOLATION: Tool '{tool_name}' requires forbidden parameter '{req_str}'!"
                    );
                }
            }
        }

        // 3. Check property descriptions: verify they do NOT ask the agent to supply a workspace path
        if let Some(props_obj) = props.as_object() {
            for (prop_name, prop_val) in props_obj {
                if let Some(desc) = prop_val["description"].as_str() {
                    let desc_lower = desc.to_lowercase();
                    assert!(
                        !desc_lower.contains("pass workspace root")
                            && !desc_lower.contains("provide workspace path"),
                        "Tool '{tool_name}' property '{prop_name}' description prompts for workspace: {desc}"
                    );
                }
            }
        }
    }

    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_adversarial_worktree_parent_in_active_transaction_resilience() {
    let temp_dir = code_kb_core::safe_tempdir();
    let main_root = temp_dir.path().join("main_repo");
    let wt_root = main_root.join(".worktrees").join("feature-active-tx");

    fs::create_dir_all(main_root.join(".git")).unwrap();
    fs::create_dir_all(main_root.join(".code-kb")).unwrap();
    fs::create_dir_all(main_root.join("src")).unwrap();

    let main_file = main_root.join("src").join("main.rs");
    let main_code = "pub fn tx_resilience_fn() {}\n";
    fs::write(&main_file, main_code).unwrap();

    let main_db = main_root.join(".code-kb").join("artifact.db");
    let mut conn = code_kb_core::open_read_write(&main_db).unwrap();
    create_full_schema(&conn);

    let h = format!("blake3:{}", blake3::hash(main_code.as_bytes()).to_hex());
    conn.execute(
        "INSERT INTO files VALUES ('f1', 'src/main.rs', 'rust', ?1, ?2, 1, '2026-01-01')",
        rusqlite::params![h, main_code.len() as i64],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO symbols VALUES (
            's1', 'f1', 'src/main.rs', 'rust', 'tx_resilience_fn', 'function',
            'pub fn tx_resilience_fn()', NULL, 'pub', NULL,
            1, 0, 1, 28, 0, 28, 1, 0, 1, 28, 0, 28, 'b3:hash',
            NULL, 0, 0
        )",
        [],
    )
    .unwrap();

    // Begin an active EXCLUSIVE write transaction on parent DB that is NOT committed yet
    let tx = conn
        .transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)
        .unwrap();
    tx.execute("INSERT INTO files VALUES ('f_uncommitted', 'src/uncommitted.rs', 'rust', 'hash', 10, 1, '2026-01-01')", []).unwrap();

    // Set up worktree
    fs::create_dir_all(wt_root.join("src")).unwrap();
    let gitdir_path = main_root
        .join(".git")
        .join("worktrees")
        .join("feature-active-tx");
    fs::create_dir_all(&gitdir_path).unwrap();
    fs::write(
        wt_root.join(".git"),
        format!("gitdir: {}\n", gitdir_path.display()),
    )
    .unwrap();
    let wt_file = wt_root.join("src").join("main.rs");
    fs::write(&wt_file, main_code).unwrap();

    let _wt_db = wt_root.join(".code-kb").join("artifact.db");

    // Spawn server - must not panic, deadlock, or crash when parent DB has an active write transaction
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("serve")
            .arg("--root")
            .arg(&main_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn code-kb serve"),
    );

    let mut stdin = child.stdin.take().expect("Failed to open stdin");
    let stdout = child.stdout.take().expect("Failed to open stdout");
    let mut reader = BufReader::new(stdout);

    // Initialize
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "challenger-test", "version": "1.0" }
        }
    });
    let mut line = serde_json::to_string(&init_req).unwrap();
    line.push('\n');
    stdin.write_all(line.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).unwrap();

    // Query on worktree file
    let wt_call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "file_skeleton",
            "arguments": { "file": wt_file.to_string_lossy().to_string() }
        }
    });
    let mut line2 = serde_json::to_string(&wt_call).unwrap();
    line2.push('\n');
    stdin.write_all(line2.as_bytes()).unwrap();
    stdin.flush().unwrap();
    let mut resp_line2 = String::new();
    reader.read_line(&mut resp_line2).unwrap();
    let resp2: Value = serde_json::from_str(&resp_line2).unwrap();
    assert_eq!(resp2["id"], 2);

    // Server must respond cleanly without hang or crash
    assert!(resp2["result"]["content"][0]["text"].as_str().is_some());

    // Rollback active transaction and clean up
    drop(tx);
    drop(conn);
    drop(stdin);
    let _ = child.wait();
}

#[test]
fn test_adversarial_watcher_directory_deletion_multilevel_cascades() {
    let temp_dir = code_kb_core::safe_tempdir();
    let db_path = temp_dir.path().join("multilevel.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();

    conn.execute_batch("CREATE TABLE files (path TEXT PRIMARY KEY);")
        .unwrap();

    // Insert 4-level deep structure
    let file_paths = [
        "top/mid1/sub1/a.rs",
        "top/mid1/sub1/b.rs",
        "top/mid1/sub2/c.rs",
        "top/mid2/sub3/d.rs",
        "top/file_at_top.rs",
        "other_top/sub/e.rs",
    ];
    for p in &file_paths {
        conn.execute("INSERT INTO files VALUES (?1);", [p]).unwrap();
    }

    let query_children = |dir: &str| -> Vec<String> {
        let escaped = dir
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let pattern = format!("{escaped}/%");
        let mut stmt = conn
            .prepare("SELECT path FROM files WHERE path LIKE ?1 ESCAPE '\\'")
            .unwrap();
        let rows = stmt
            .query_map([&pattern], |r| r.get::<_, String>(0))
            .unwrap();
        rows.flatten().collect()
    };

    // Deleting "top/mid1" should find only a.rs, b.rs, c.rs (3 files)
    let mid1_children = query_children("top/mid1");
    assert_eq!(mid1_children.len(), 3);
    assert!(mid1_children.contains(&"top/mid1/sub1/a.rs".to_string()));
    assert!(mid1_children.contains(&"top/mid1/sub1/b.rs".to_string()));
    assert!(mid1_children.contains(&"top/mid1/sub2/c.rs".to_string()));
    assert!(!mid1_children.contains(&"top/mid2/sub3/d.rs".to_string()));
    assert!(!mid1_children.contains(&"top/file_at_top.rs".to_string()));

    // Deleting "top" should find all 5 files under top/ but NOT other_top/
    let top_children = query_children("top");
    assert_eq!(top_children.len(), 5);
    assert!(!top_children.contains(&"other_top/sub/e.rs".to_string()));
}
