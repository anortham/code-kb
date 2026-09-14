use std::path::Path;
use std::process::Command;

fn setup_adversarial_test_repo() -> tempfile::TempDir {
    let temp_dir = code_kb_core::safe_tempdir();
    let root = temp_dir.path().to_path_buf();

    let src_dir = root.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let content = "pub struct Workspace {\n    pub root: String,\n}\n\npub fn run_task() {\n    helper();\n}\n\nfn helper() {}\n";
    std::fs::write(src_dir.join("workspace.rs"), content).unwrap();

    let tests_dir = root.join("tests");
    std::fs::create_dir_all(&tests_dir).unwrap();
    let test_content = "#[test]\nfn test_run_task() {\n    run_task();\n}\n";
    std::fs::write(tests_dir.join("test_workspace.rs"), test_content).unwrap();

    // Use code-kb scan to initialize a genuine schema with artifact_metadata
    let scan_out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
        .arg("--root")
        .arg(&root)
        .arg("scan")
        .output()
        .expect("Failed to initialize test repo via scan");
    assert!(
        scan_out.status.success(),
        "scan failed: {}",
        String::from_utf8_lossy(&scan_out.stderr)
    );

    // Populate a sample structural fact and literal so facts returns non-empty data with paths
    let db_path = root.join(".code-kb").join("artifact.db");
    let conn = code_kb_core::open_read_write(&db_path).unwrap();
    let file_id: String = conn
        .query_row(
            "SELECT file_id FROM files WHERE path LIKE '%workspace.rs' LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();

    conn.execute(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id, capture_name, node_kind, containing_symbol_id, start_line, start_column, end_line, end_column, start_byte, end_byte, confidence)
         VALUES ('sf1', ?1, 'src/workspace.rs', 'rust', 'route', 'get_index', 'route', NULL, 1, 0, 10, 1, 0, 100, 1.0)",
        [&file_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO literals (literal_id, file_id, path, language, kind, literal_text, carrier, arg_position, containing_symbol_id, start_line, start_column, end_line, end_column, start_byte, end_byte, confidence)
         VALUES ('lit1', ?1, 'src/workspace.rs', 'rust', 'string', 'hello', 'identifier', 0, NULL, 1, 0, 1, 5, 0, 5, 1.0)",
        [&file_id],
    )
    .unwrap();

    temp_dir
}

fn invert_drive_casing(p: &Path) -> String {
    let s = p.to_string_lossy().to_string();
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        let first = s.chars().next().unwrap();
        let toggled = if first.is_ascii_uppercase() {
            first.to_ascii_lowercase()
        } else {
            first.to_ascii_uppercase()
        };
        format!("{}{}", toggled, &s[1..])
    } else {
        s
    }
}

// ============================================================================
// CHALLENGE 1: CLI Argument Path Variations Matrix
// ============================================================================
#[test]
fn test_adversarial_cli_path_variations_across_all_commands() {
    let repo = setup_adversarial_test_repo();
    let root = repo.path();
    let root_str = root.to_string_lossy().to_string();
    let inverted_root = invert_drive_casing(root);

    let abs_file = root.join("src").join("workspace.rs");
    let abs_file_str = abs_file.to_string_lossy().to_string();
    let inverted_abs_file = invert_drive_casing(&abs_file);
    let forward_abs_file = abs_file_str.replace('\\', "/");
    let file_uri_3slash = format!("file:///{}", forward_abs_file);
    let file_uri_2slash = format!("file://{}", forward_abs_file);
    let file_uri_inverted = format!("file:///{}", inverted_abs_file.replace('\\', "/"));

    let path_variations = vec![
        r"src\workspace.rs",            // native Windows backslash
        r"src/workspace.rs",            // forward slash
        r".\src/workspace.rs",          // mixed backslash and forward slash
        r"./src\workspace.rs",          // mixed forward slash and backslash
        r".\src\workspace.rs",          // dot with backslash
        r"./src/workspace.rs",          // dot with forward slash
        r"src/./workspace.rs",          // internal single dot forward
        r"src\.\workspace.rs",          // internal single dot backslash
        r"src/../src/workspace.rs",     // parent traversal forward
        r"src\..\src\workspace.rs",     // parent traversal backslash
        r".\src\..\src\.\workspace.rs", // complex mixed dots
        &abs_file_str,                  // native absolute path
        &inverted_abs_file,             // inverted drive letter absolute path
        &forward_abs_file,              // forward slash absolute path
        &file_uri_3slash,               // file:/// URI standard
        &file_uri_2slash,               // file:// URI two-slash
        &file_uri_inverted,             // file:/// URI with inverted drive letter
    ];

    for path_var in &path_variations {
        // 1. Skeleton
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("skeleton")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed skeleton for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "skeleton failed for path variation '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Workspace") || stdout.contains("run_task"),
            "skeleton missing symbols for '{path_var}':\n{stdout}"
        );

        // 2. Symbol with --path filter
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("symbol")
            .arg("Workspace")
            .arg("--path")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed symbol for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "symbol failed for path filter '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Workspace"),
            "symbol missing result for '{path_var}':\n{stdout}"
        );

        // 3. Search with --path filter
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("search")
            .arg("task")
            .arg("--path")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed search for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "search failed for path filter '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("run_task"),
            "search missing result for '{path_var}':\n{stdout}"
        );

        // 4. Body with --file
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("body")
            .arg("run_task")
            .arg("--file")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed body for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "body failed for file '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("helper()"),
            "body missing implementation for '{path_var}':\n{stdout}"
        );

        // 5. Slice with --file
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("slice")
            .arg("run_task")
            .arg("--file")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed slice for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "slice failed for file '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("helper"),
            "slice missing callee for '{path_var}':\n{stdout}"
        );

        // 6. Blast radius with --file
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("blast-radius")
            .arg("helper")
            .arg("--file")
            .arg(path_var)
            .output()
            .unwrap_or_else(|e| panic!("Failed blast-radius for '{path_var}': {e}"));
        assert!(
            out.status.success(),
            "blast-radius failed for file '{path_var}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("run_task"),
            "blast-radius missing impacted symbol for '{path_var}':\n{stdout}"
        );
    }

    // 7. Outline with scoped path variations
    let outline_path_vars = vec![
        r"src",
        r"src\",
        r"src/",
        r".\src",
        r"./src",
        r"src\..\src",
        &inverted_root,
    ];
    for opv in outline_path_vars {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("outline")
            .arg("-p")
            .arg(opv)
            .output()
            .unwrap_or_else(|e| panic!("Failed outline -p '{opv}': {e}"));
        assert!(
            out.status.success(),
            "outline failed for path filter '{opv}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // 8. Global root casing, slashes, and trailing separators
    let root_variations = vec![
        root_str.clone(),
        inverted_root.clone(),
        format!(r"{}\", root_str.trim_end_matches('\\')),
        format!("{}/", root_str.replace('\\', "/")),
    ];
    for rv in root_variations {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(&rv)
            .arg("symbol")
            .arg("Workspace")
            .output()
            .unwrap_or_else(|e| panic!("Failed symbol with root '{rv}': {e}"));
        assert!(
            out.status.success(),
            "symbol failed with root '{rv}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // 9. Atomic edit with backslash, mixed slash, dot traversal, and absolute paths
    let edit_file_vars = [
        r"src\workspace.rs",
        r".\src/workspace.rs",
        r"src/../src/workspace.rs",
        &abs_file_str,
        &inverted_abs_file,
    ];
    for (i, ef) in edit_file_vars.iter().enumerate() {
        let new_body = format!("{{\n    // edit variation {}\n}}", i);
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .arg("edit")
            .arg("helper")
            .arg("--file")
            .arg(ef)
            .arg("--body")
            .arg(&new_body)
            .output()
            .unwrap_or_else(|e| panic!("Failed edit for '{ef}': {e}"));
        assert!(
            out.status.success(),
            "edit failed for file path variation '{ef}':\nstderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("Successfully replaced body"),
            "edit missing success message for '{ef}':\n{stdout}"
        );
    }
}

// ============================================================================
// CHALLENGE 2: Strict Forward-Slash Assertions on CLI --json Output
// ============================================================================
#[test]
fn test_adversarial_cli_json_strict_forward_slash_across_all_commands() {
    let repo = setup_adversarial_test_repo();
    let root = repo.path();
    let abs_file = root.join("src").join("workspace.rs");
    let abs_file_str = abs_file.to_string_lossy().to_string();
    let file_uri_3slash = format!("file:///{}", abs_file_str.replace('\\', "/"));

    fn assert_all_paths_forward_slash(val: &serde_json::Value, found_paths: &mut usize) {
        match val {
            serde_json::Value::Object(map) => {
                for (k, v) in map {
                    let k_lower = k.to_ascii_lowercase();
                    if (k_lower.contains("path") || k_lower.contains("file")) && v.is_string() {
                        let s = v.as_str().unwrap();
                        assert!(
                            !s.contains('\\'),
                            "Key '{k}' contains backslash in JSON output: '{s}'"
                        );
                        assert!(
                            !s.starts_with(r"\\?\"),
                            "Key '{k}' contains verbatim prefix in JSON output: '{s}'"
                        );
                        *found_paths += 1;
                    }
                    assert_all_paths_forward_slash(v, found_paths);
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    assert_all_paths_forward_slash(item, found_paths);
                }
            }
            _ => {}
        }
    }

    // Commands tested specifically with backslashes, mixed slashes, and dots in their arguments
    let mut test_cases: Vec<(Vec<String>, bool)> = vec![
        // (cmd_args, expect_paths)
        (vec!["--json".into(), "outline".into()], true),
        (
            vec![
                "--json".into(),
                "outline".into(),
                "-p".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "outline".into(),
                "-p".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "skeleton".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "skeleton".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "skeleton".into(),
                r"src/../src/workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "symbol".into(),
                "Workspace".into(),
                "--path".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "symbol".into(),
                "Workspace".into(),
                "--path".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "search".into(),
                "Workspace".into(),
                "--path".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "body".into(),
                "run_task".into(),
                "--file".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "body".into(),
                "run_task".into(),
                "--file".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "slice".into(),
                "run_task".into(),
                "--file".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "slice".into(),
                "run_task".into(),
                "--file".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (vec!["--json".into(), "refs".into(), "helper".into()], true),
        (
            vec![
                "--json".into(),
                "blast-radius".into(),
                "helper".into(),
                "--file".into(),
                r"src\workspace.rs".into(),
            ],
            true,
        ),
        (
            vec![
                "--json".into(),
                "blast-radius".into(),
                "helper".into(),
                "--file".into(),
                r".\src/workspace.rs".into(),
            ],
            true,
        ),
        (vec!["--json".into(), "facts".into()], false), // Category listing has no path fields
        (vec!["--json".into(), "facts".into(), "route".into()], true), // Facts query returns structural_facts and literals with paths
        (
            vec![
                "--json".into(),
                "edit".into(),
                "helper".into(),
                "--file".into(),
                r"src\workspace.rs".into(),
                "--body".into(),
                "{\n    // json edit test\n}".into(),
            ],
            true,
        ),
        (vec!["--json".into(), "stats".into()], false), // Stats output has counts, no paths
    ];

    // File URI and absolute path JSON queries
    test_cases.push((
        vec!["--json".into(), "skeleton".into(), file_uri_3slash.clone()],
        true,
    ));
    test_cases.push((
        vec!["--json".into(), "skeleton".into(), abs_file_str.clone()],
        true,
    ));
    test_cases.push((
        vec![
            "--json".into(),
            "body".into(),
            "run_task".into(),
            "--file".into(),
            file_uri_3slash.clone(),
        ],
        true,
    ));
    test_cases.push((
        vec![
            "--json".into(),
            "slice".into(),
            "run_task".into(),
            "--file".into(),
            file_uri_3slash.clone(),
        ],
        true,
    ));
    test_cases.push((
        vec![
            "--json".into(),
            "symbol".into(),
            "Workspace".into(),
            "--path".into(),
            file_uri_3slash.clone(),
        ],
        true,
    ));
    test_cases.push((
        vec![
            "--json".into(),
            "search".into(),
            "Workspace".into(),
            "--path".into(),
            file_uri_3slash.clone(),
        ],
        true,
    ));
    test_cases.push((
        vec![
            "--json".into(),
            "blast-radius".into(),
            "helper".into(),
            "--file".into(),
            file_uri_3slash,
        ],
        true,
    ));

    for (args, expect_paths) in test_cases {
        let out = Command::new(env!("CARGO_BIN_EXE_code-kb"))
            .arg("--root")
            .arg(root)
            .args(&args)
            .output()
            .unwrap_or_else(|e| panic!("Failed to execute {:?}: {e}", args));
        assert!(
            out.status.success(),
            "Command failed: {:?}\nstderr: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );

        let val: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
            panic!(
                "Failed to parse JSON for {:?}: {e}\nstdout: {}",
                args,
                String::from_utf8_lossy(&out.stdout)
            )
        });

        let mut found_paths = 0;
        assert_all_paths_forward_slash(&val, &mut found_paths);

        if expect_paths {
            assert!(
                found_paths > 0,
                "Expected at least 1 path field in JSON for command {:?}, but found 0.\nJSON: {}",
                args,
                serde_json::to_string_pretty(&val).unwrap()
            );
        }
    }
}

// ============================================================================
// CHALLENGE 3: Workspace strip_prefix_lossy and UNC Path Equality
// ============================================================================
#[test]
fn test_adversarial_strip_prefix_lossy_and_unc_matrix() {
    use code_kb_core::workspace::{paths_equal, strip_prefix_lossy};

    #[cfg(windows)]
    {
        // --- 1. UNC paths_equal matrix ---
        // Case insensitivity across server, share, and path components
        assert!(paths_equal(
            Path::new(r"\\server\share\file.rs"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\server\share\file.rs"),
            Path::new(r"\\SERVER\SHARE\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\server\share\folder\FILE.RS"),
            Path::new(r"\\SERVER\SHARE\FOLDER\file.rs")
        ));

        // Mixed slashes in UNC
        assert!(paths_equal(
            Path::new("//server/share/file.rs"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new("//SERVER/SHARE/folder/file.rs"),
            Path::new(r"\\server\share\folder\file.rs")
        ));

        // Verbatim UNC (\\?\UNC\server\share) vs standard UNC
        assert!(paths_equal(
            Path::new(r"\\?\UNC\server\share\file.rs"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\UNC\SERVER\SHARE\file.rs"),
            Path::new(r"\\server\share\file.rs")
        ));
        assert!(paths_equal(
            Path::new(r"\\?\UNC\server\share\file.rs"),
            Path::new(r"\\SERVER\SHARE\file.rs")
        ));

        // Trailing slash on UNC directories
        assert!(paths_equal(
            Path::new(r"\\server\share\dir\"),
            Path::new(r"\\server\share\dir")
        ));

        // Negative UNC tests (different servers, different shares, different paths)
        assert!(!paths_equal(
            Path::new(r"\\server1\share\file.rs"),
            Path::new(r"\\server2\share\file.rs")
        ));
        assert!(!paths_equal(
            Path::new(r"\\server\share1\file.rs"),
            Path::new(r"\\server\share2\file.rs")
        ));
        assert!(!paths_equal(
            Path::new(r"\\server\share\file1.rs"),
            Path::new(r"\\server\share\file2.rs")
        ));

        // --- 2. UNC strip_prefix_lossy matrix ---
        let unc_base = Path::new(r"\\server\share\repo");
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\server\share\repo\src\lib.rs"), unc_base),
            Some(Path::new(r"src\lib.rs"))
        );
        // Casing in server/share
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\SERVER\SHARE\repo\src\lib.rs"), unc_base),
            Some(Path::new(r"src\lib.rs"))
        );
        // Casing in repo folder
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\SERVER\SHARE\REPO\src\lib.rs"), unc_base),
            Some(Path::new(r"src\lib.rs"))
        );
        // Base with trailing slash
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"\\server\share\repo\src\lib.rs"),
                Path::new(r"\\server\share\repo\")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        // Verbatim UNC path
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\?\UNC\server\share\repo\src\lib.rs"), unc_base),
            Some(Path::new(r"src\lib.rs"))
        );
        // Verbatim UNC base
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"\\server\share\repo\src\lib.rs"),
                Path::new(r"\\?\UNC\server\share\repo")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        // Negative UNC non-matching
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\server\other_share\repo\src\lib.rs"), unc_base),
            None
        );
        assert_eq!(
            strip_prefix_lossy(Path::new(r"\\other_server\share\repo\src\lib.rs"), unc_base),
            None
        );

        // --- 3. Windows drive roots strip_prefix_lossy matrix ---
        let drive_root = Path::new(r"C:\");
        assert_eq!(
            strip_prefix_lossy(Path::new(r"C:\src\lib.rs"), drive_root),
            Some(Path::new(r"src\lib.rs"))
        );
        // Drive letter casing on drive root
        assert_eq!(
            strip_prefix_lossy(Path::new(r"c:\src\lib.rs"), drive_root),
            Some(Path::new(r"src\lib.rs"))
        );
        // Inverted drive root base
        assert_eq!(
            strip_prefix_lossy(Path::new(r"C:\src\lib.rs"), Path::new(r"c:\")),
            Some(Path::new(r"src\lib.rs"))
        );
        // Root stripped from itself
        assert_eq!(
            strip_prefix_lossy(Path::new(r"C:\"), drive_root),
            Some(Path::new(""))
        );

        // --- 4. Verbatim disk prefixes strip_prefix_lossy ---
        let win_base = Path::new(r"C:\source\code-kb");
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"C:\source\code-kb\src\lib.rs"),
                Path::new(r"\\?\C:\source\code-kb")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"C:\source\code-kb\src\lib.rs"),
                Path::new(r"\\?\c:\source\code-kb")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"\\?\C:\source\code-kb\src\lib.rs"),
                Path::new(r"\\?\c:\source\code-kb")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        // Additional paths_equal drive root and trailing slash invariants
        assert!(paths_equal(Path::new(r"c:\repo\"), Path::new(r"C:\repo")));
        assert!(paths_equal(Path::new(r"c:\"), Path::new(r"C:\")));
        assert!(paths_equal(
            Path::new(r"\\server\share\"),
            Path::new(r"\\SERVER\SHARE")
        ));

        // UNC directory casing in strip_prefix_lossy
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"\\server\share\MY_DIR\file.rs"),
                Path::new(r"\\server\share\my_dir")
            ),
            Some(Path::new(r"file.rs"))
        );

        // Verbatim base with trailing slash
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"\\?\C:\source\code-kb\src\lib.rs"),
                Path::new(r"\\?\C:\source\code-kb\")
            ),
            Some(Path::new(r"src\lib.rs"))
        );
        assert_eq!(
            strip_prefix_lossy(
                Path::new(r"C:\source\code-kb\src\lib.rs"),
                Path::new(r"\\?\c:\source\code-kb\")
            ),
            Some(Path::new(r"src\lib.rs"))
        );

        // Shorter path than base
        assert_eq!(strip_prefix_lossy(Path::new(r"C:\source"), win_base), None);
    }

    // Relative path tests (all platforms)
    assert_eq!(
        strip_prefix_lossy(Path::new("src/lib.rs"), Path::new("src")),
        Some(Path::new("lib.rs"))
    );
    assert_eq!(
        strip_prefix_lossy(Path::new("src/lib.rs"), Path::new("src/")),
        Some(Path::new("lib.rs"))
    );
    assert_eq!(
        strip_prefix_lossy(Path::new("src/lib.rs"), Path::new("other")),
        None
    );
}
