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
