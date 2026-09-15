use code_kb_core::{
    Workspace, find_julie_extract_binary, open_read_only, reconcile_offline_edits, safe_tempdir,
    scan_workspace, search_symbols,
};
use std::fs;
use std::process::Command;

#[test]
fn test_git_worktree_lifecycle_and_index_isolation() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    // 1. Initialize git repository
    let run_git = |args: &[&str], cwd: &std::path::Path| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("Failed to run git {args:?}: {e}"));
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    run_git(&["init"], &root);
    run_git(&["config", "core.autocrlf", "false"], &root);
    run_git(&["config", "user.name", "Test Agent"], &root);
    run_git(&["config", "user.email", "agent@example.com"], &root);

    // Write .gitignore with .worktrees/ and .code-kb/
    fs::write(
        root.join(".gitignore"),
        ".worktrees/\nworktrees/\n.code-kb/\n*.db\n",
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "pub fn main_app_func() {}\n").unwrap();

    run_git(&["add", "."], &root);
    run_git(&["commit", "-m", "Initial commit"], &root);

    // 2. Scan main workspace
    let main_ws = Workspace::new(root.clone());
    let main_db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&main_ws, &main_db, true).expect("Main workspace scan failed");

    {
        let conn = open_read_only(&main_db).expect("Failed to open main db");
        let syms = search_symbols(&conn, "main_app_func", None, false, 10).unwrap();
        assert_eq!(syms.len(), 1, "main_app_func must exist in main index");
    }

    // 3. Create a git worktree in .worktrees/wt-feature
    let wt_dir = root.join(".worktrees").join("wt-feature");
    run_git(
        &[
            "worktree",
            "add",
            wt_dir.to_str().unwrap(),
            "-b",
            "wt-feature-branch",
        ],
        &root,
    );

    // Add a new feature file in the worktree
    let wt_src = wt_dir.join("src");
    fs::write(wt_src.join("feature.rs"), "pub fn feature_func() {}\n").unwrap();

    // 4. Discover and scan the worktree workspace
    let wt_ws = Workspace::discover(Some(&wt_dir)).expect("Discovering worktree workspace failed");
    assert_eq!(wt_ws.canonical_root, dunce::canonicalize(&wt_dir).unwrap());

    let wt_db = wt_dir.join(".code-kb").join("artifact.db");
    scan_workspace(&wt_ws, &wt_db, true).expect("Worktree scan failed");

    // Verify worktree index has both main_app_func and feature_func
    {
        let conn = open_read_only(&wt_db).expect("Failed to open worktree db");
        let syms = search_symbols(&conn, "feature_func", None, false, 10).unwrap();
        assert_eq!(syms.len(), 1, "feature_func must exist in worktree index");

        let main_syms = search_symbols(&conn, "main_app_func", None, false, 10).unwrap();
        assert_eq!(
            main_syms.len(),
            1,
            "main_app_func must also exist in worktree index"
        );
    }

    // 5. Test file modifications and freshness isolation between main and worktree
    {
        // Edit file in worktree
        fs::write(
            wt_src.join("feature.rs"),
            "pub fn feature_func() {}\npub fn wt_extra_func() {}\n",
        )
        .unwrap();

        let wt_conn = open_read_only(&wt_db).expect("Failed to open worktree db");
        let updated = code_kb_core::ensure_fresh_file(&wt_ws, &wt_db, &wt_conn, "src/feature.rs")
            .expect("Freshness check on worktree file failed");
        assert!(
            updated,
            "ensure_fresh_file must detect modification in worktree"
        );

        let wt_conn2 = open_read_only(&wt_db).expect("Failed to reopen worktree db");
        let wt_syms = search_symbols(&wt_conn2, "wt_extra_func", None, false, 10).unwrap();
        assert_eq!(
            wt_syms.len(),
            1,
            "wt_extra_func must exist in worktree index after edit"
        );

        // Verify main repo index does NOT contain wt_extra_func
        let main_conn = open_read_only(&main_db).expect("Failed to open main db");
        let main_syms = search_symbols(&main_conn, "wt_extra_func", None, false, 10).unwrap();
        assert!(
            main_syms.is_empty(),
            "Main repo index must NOT contain wt_extra_func from worktree"
        );
    }

    {
        // Edit file in main repo
        fs::write(
            src_dir.join("main.rs"),
            "pub fn main_app_func() {}\npub fn main_extra_func() {}\n",
        )
        .unwrap();

        let main_conn = open_read_only(&main_db).expect("Failed to open main db");
        let updated =
            code_kb_core::ensure_fresh_file(&main_ws, &main_db, &main_conn, "src/main.rs")
                .expect("Freshness check on main file failed");
        assert!(
            updated,
            "ensure_fresh_file must detect modification in main repo"
        );

        let main_conn2 = open_read_only(&main_db).expect("Failed to reopen main db");
        let main_syms = search_symbols(&main_conn2, "main_extra_func", None, false, 10).unwrap();
        assert_eq!(
            main_syms.len(),
            1,
            "main_extra_func must exist in main repo index after edit"
        );

        // Verify worktree index does NOT contain main_extra_func
        let wt_conn = open_read_only(&wt_db).expect("Failed to open worktree db");
        let wt_syms = search_symbols(&wt_conn, "main_extra_func", None, false, 10).unwrap();
        assert!(
            wt_syms.is_empty(),
            "Worktree index must NOT contain main_extra_func from main repo"
        );
    }

    // 6. Verify main repo isolation: main index must NOT be polluted by worktree
    {
        let conn = open_read_only(&main_db).expect("Failed to open main db");
        let report =
            reconcile_offline_edits(&main_ws, &main_db, &conn).expect("Main repo reconcile failed");

        assert!(
            report.added.is_empty(),
            "Main repo reconcile must not detect any added files from .worktrees: {:?}",
            report.added
        );

        let syms = search_symbols(&conn, "feature_func", None, false, 10).unwrap();
        assert!(
            syms.is_empty(),
            "Main repo index must NOT contain feature_func from worktree"
        );
    }

    // 6. Remove the git worktree and clean up
    run_git(
        &["worktree", "remove", wt_dir.to_str().unwrap(), "--force"],
        &root,
    );
    assert!(!wt_dir.exists(), "Worktree directory must be removed");

    // 7. Verify main repo index is completely intact after worktree removal
    {
        let conn = open_read_only(&main_db).expect("Failed to open main db");
        let report = reconcile_offline_edits(&main_ws, &main_db, &conn)
            .expect("Main repo reconcile failed after worktree removal");

        assert!(
            report.deleted.is_empty(),
            "Main repo reconcile must not delete anything after worktree removal: {:?}",
            report.deleted
        );

        let syms = search_symbols(&conn, "main_app_func", None, false, 10).unwrap();
        assert_eq!(
            syms.len(),
            1,
            "main_app_func must still exist intact in main index"
        );
    }
}

#[test]
fn test_worktree_copy_index_update_and_query_workflow() {
    let _extract_bin =
        find_julie_extract_binary().expect("julie-extract binary must be present for tests");

    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let run_git = |args: &[&str], cwd: &std::path::Path| {
        let output = Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap_or_else(|e| panic!("Failed to run git {args:?}: {e}"));
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    };

    // 1. Initialize git repository
    run_git(&["init", "-b", "main"], &root);
    run_git(&["config", "core.autocrlf", "false"], &root);
    run_git(&["config", "user.name", "Test User"], &root);
    run_git(&["config", "user.email", "test@example.com"], &root);

    // .gitattributes to ensure git checkouts preserve exact bytes across platforms
    fs::write(root.join(".gitattributes"), "* -text\n").unwrap();

    // .gitignore
    fs::write(
        root.join(".gitignore"),
        ".worktrees/\nworktrees/\n.code-kb/\n*.db\n",
    )
    .unwrap();

    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).unwrap();
    fs::write(src_dir.join("main.rs"), "pub fn main_app_func() {}\n").unwrap();
    fs::write(src_dir.join("lib.rs"), "pub fn lib_func() {}\n").unwrap();
    fs::write(src_dir.join("utils.rs"), "pub fn util_func() {}\n").unwrap();

    run_git(&["add", "."], &root);
    run_git(&["commit", "-m", "Initial commit on main"], &root);

    // 2. Build initial parent index
    let main_ws = Workspace::new(root.clone());
    let main_db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&main_ws, &main_db, true).expect("Main workspace scan failed");

    // Flush WAL to make sure main_db contains all transactions before copy
    {
        let parent_conn = code_kb_core::open_read_write(&main_db).unwrap();
        code_kb_core::db::checkpoint_truncate(&parent_conn).unwrap();
    }

    // Verify initial symbols exist in main repo
    {
        let conn = open_read_only(&main_db).expect("Failed to open main db");
        assert_eq!(
            search_symbols(&conn, "main_app_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_symbols(&conn, "lib_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_symbols(&conn, "util_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
    }

    // 3. Step 1 of workflow: Create git worktree
    let wt_dir = root.join(".worktrees").join("wt-copy");
    run_git(
        &[
            "worktree",
            "add",
            wt_dir.to_str().unwrap(),
            "-b",
            "feature-copy-branch",
        ],
        &root,
    );
    assert!(wt_dir.exists(), "Worktree directory must exist");

    // 4. Step 2 of workflow: Copy index from parent repo to worktree
    let wt_db = wt_dir.join(".code-kb").join("artifact.db");
    fs::create_dir_all(wt_dir.join(".code-kb")).unwrap();
    fs::copy(&main_db, &wt_db).expect("Copying main artifact.db to worktree must succeed");
    code_kb_core::db::ensure_fts_index_path(&wt_db).expect("Ensuring FTS index must succeed");
    assert!(wt_db.exists(), "Worktree DB must exist after copy");

    let wt_ws = Workspace::discover(Some(&wt_dir)).expect("Discovering worktree workspace failed");

    // Verify that immediately after copying, the worktree DB is valid and queryable
    {
        let conn = open_read_only(&wt_db).expect("Failed to open copied worktree db");
        assert_eq!(
            search_symbols(&conn, "main_app_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_symbols(&conn, "lib_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_symbols(&conn, "util_func", None, false, 10)
                .unwrap()
                .len(),
            1
        );
    }

    // 5. Step 3 of workflow: Update worktree files
    let wt_src = wt_dir.join("src");

    // Add a new file in worktree
    fs::write(
        wt_src.join("new_module.rs"),
        "pub fn new_module_func() {}\n",
    )
    .unwrap();

    // Modify existing file in worktree
    fs::write(
        wt_src.join("main.rs"),
        "pub fn main_app_func() {}\npub fn wt_modified_func() {}\n",
    )
    .unwrap();

    // Delete a file in worktree
    fs::remove_file(wt_src.join("utils.rs")).unwrap();

    // Reconcile worktree offline edits (simulates MCP auto-reconciliation or background watcher)
    {
        let conn = open_read_only(&wt_db).expect("Failed to open worktree db for reconcile");
        let report = reconcile_offline_edits(&wt_ws, &wt_db, &conn)
            .expect("Worktree reconcile must succeed");

        assert_eq!(report.added, vec!["src/new_module.rs"]);
        assert_eq!(report.modified, vec!["src/main.rs"]);
        assert_eq!(report.deleted, vec!["src/utils.rs"]);
    }

    // 6. Step 4 of workflow: Query the updated worktree index
    {
        let conn = open_read_only(&wt_db).expect("Failed to reopen worktree db after reconcile");

        // Newly added symbol is present
        let new_syms = search_symbols(&conn, "new_module_func", None, false, 10).unwrap();
        assert_eq!(
            new_syms.len(),
            1,
            "new_module_func must exist in worktree index"
        );
        assert_eq!(new_syms[0].path, "src/new_module.rs");

        // Modified file contains new symbol
        let mod_syms = search_symbols(&conn, "wt_modified_func", None, false, 10).unwrap();
        assert_eq!(
            mod_syms.len(),
            1,
            "wt_modified_func must exist in worktree index"
        );
        assert_eq!(mod_syms[0].path, "src/main.rs");

        // Deleted file symbol is removed
        let del_syms = search_symbols(&conn, "util_func", None, false, 10).unwrap();
        assert!(
            del_syms.is_empty(),
            "util_func from deleted file must be gone from worktree index"
        );

        // Untouched file symbol remains intact
        let lib_syms = search_symbols(&conn, "lib_func", None, false, 10).unwrap();
        assert_eq!(
            lib_syms.len(),
            1,
            "lib_func must still exist in worktree index"
        );
    }

    // 7. Step 5 of workflow: Test JIT freshness guard on subsequent single-file edit in worktree
    {
        // Add another function to new_module.rs
        fs::write(
            wt_src.join("new_module.rs"),
            "pub fn new_module_func() {}\npub fn second_wt_func() {}\n",
        )
        .unwrap();

        let conn = open_read_only(&wt_db).expect("Failed to open worktree db for freshness check");
        let updated = code_kb_core::ensure_fresh_file(&wt_ws, &wt_db, &conn, "src/new_module.rs")
            .expect("Freshness check on worktree file must succeed");
        assert!(
            updated,
            "ensure_fresh_file must detect modification in worktree"
        );

        let conn2 = open_read_only(&wt_db).expect("Failed to reopen worktree db");
        let syms = search_symbols(&conn2, "second_wt_func", None, false, 10).unwrap();
        assert_eq!(
            syms.len(),
            1,
            "second_wt_func must be queryable immediately after JIT update"
        );
    }

    // 8. Step 6: Verify total isolation of parent repository index
    {
        let main_conn = open_read_only(&main_db).expect("Failed to open main db");

        // Main repo should NOT have worktree-only symbols
        assert!(
            search_symbols(&main_conn, "new_module_func", None, false, 10)
                .unwrap()
                .is_empty(),
            "Main repo must NOT contain new_module_func"
        );
        assert!(
            search_symbols(&main_conn, "wt_modified_func", None, false, 10)
                .unwrap()
                .is_empty(),
            "Main repo must NOT contain wt_modified_func"
        );
        assert!(
            search_symbols(&main_conn, "second_wt_func", None, false, 10)
                .unwrap()
                .is_empty(),
            "Main repo must NOT contain second_wt_func"
        );

        // Main repo MUST still have util_func which was only deleted in worktree
        assert_eq!(
            search_symbols(&main_conn, "util_func", None, false, 10)
                .unwrap()
                .len(),
            1,
            "Main repo must still contain util_func"
        );
    }
}
