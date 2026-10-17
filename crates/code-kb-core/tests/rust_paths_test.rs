use code_kb_core::{
    Workspace, find_julie_extract_binary, find_references_scoped, open_read_only, safe_tempdir,
    scan_workspace,
};
use std::fs;

const FIXTURE: &[(&str, &str)] = &[
    (
        "crates/a-core/src/lib.rs",
        "pub mod m;\npub mod reader;\npub mod shapes;\npub mod std;\npub mod util;\n\npub fn safe_dir() -> u32 {\n    1\n}\n\npub fn calls_self() -> u32 {\n    self::safe_dir()\n}\n",
    ),
    (
        "crates/a-core/src/util.rs",
        "pub fn calls_crate() -> u32 {\n    crate::safe_dir()\n}\n\npub fn calls_crate_module() -> u32 {\n    crate::shapes::area()\n}\n\npub fn calls_std() -> usize {\n    std::fs::read(\"x\").map(|bytes| bytes.len()).unwrap_or(0)\n}\n",
    ),
    ("crates/a-core/src/m/mod.rs", "pub mod n;\n"),
    (
        "crates/a-core/src/m/n.rs",
        "pub fn calls_super_super() -> u32 {\n    super::super::safe_dir()\n}\n",
    ),
    (
        "crates/a-core/src/shapes/mod.rs",
        "pub fn area() -> u32 {\n    4\n}\n",
    ),
    (
        "crates/a-core/src/std/fs/io.rs",
        "pub fn read() -> u32 {\n    5\n}\n",
    ),
    (
        "crates/a-core/src/reader.rs",
        "pub fn read() -> u32 {\n    6\n}\n",
    ),
    (
        "crates/a-core/tests/helpers.rs",
        "pub fn safe_dir() -> u32 {\n    7\n}\n",
    ),
    (
        "crates/a-cli/src/main.rs",
        "fn calls_crate_name() -> u32 {\n    a_core::safe_dir()\n}\n\nfn calls_crate_name_module() -> u32 {\n    a_core::shapes::area()\n}\n\nfn calls_other_crate() -> u32 {\n    other_crate::safe_dir()\n}\n\nfn calls_other_crate_module() -> u32 {\n    other_crate::shapes::area()\n}\n\nfn main() {}\n",
    ),
    (
        "crates/b/src/lib.rs",
        "pub fn safe_dir() -> u32 {\n    2\n}\n\npub fn b_calls_crate() -> u32 {\n    crate::safe_dir()\n}\n\npub fn b_calls_self() -> u32 {\n    self::safe_dir()\n}\n",
    ),
    (
        "crates/b/src/other.rs",
        "pub fn b_calls_super() -> u32 {\n    super::safe_dir()\n}\n",
    ),
];

fn scanned_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in FIXTURE {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root), &db_path, true).expect("scan failed");
    (temp_dir, db_path)
}

fn callers(db: &std::path::Path, name: &str, definition_path: &str) -> Vec<String> {
    let conn = open_read_only(db).unwrap();
    let rows =
        find_references_scoped(&conn, name, "callers", 50, false, Some(definition_path)).unwrap();
    let mut names: Vec<String> = rows
        .iter()
        .map(|row| row.from_symbol_name.clone())
        .collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn crate_self_super_and_crate_name_calls_reach_the_function_of_their_own_crate() {
    let (_repo, db) = scanned_repo();

    assert_eq!(
        callers(&db, "safe_dir", "crates/a-core/src/lib.rs"),
        [
            "calls_crate",
            "calls_crate_name",
            "calls_self",
            "calls_super_super"
        ]
    );
}

#[test]
fn a_function_of_another_crate_with_the_same_name_gets_only_its_own_calls() {
    let (_repo, db) = scanned_repo();

    assert_eq!(
        callers(&db, "safe_dir", "crates/b/src/lib.rs"),
        ["b_calls_crate", "b_calls_self", "b_calls_super"]
    );
}

#[test]
fn a_module_segment_matches_a_folder_name() {
    let (_repo, db) = scanned_repo();

    assert_eq!(
        callers(&db, "area", "crates/a-core/src/shapes/mod.rs"),
        ["calls_crate_module", "calls_crate_name_module"]
    );
}

#[test]
fn a_std_path_call_does_not_reach_a_project_function_with_the_same_name() {
    let (_repo, db) = scanned_repo();

    assert!(callers(&db, "read", "crates/a-core/src/reader.rs").is_empty());
    assert!(callers(&db, "read", "crates/a-core/src/std/fs/io.rs").is_empty());
}

#[test]
fn a_function_in_the_tests_folder_is_not_reached_by_the_crate_name() {
    let (_repo, db) = scanned_repo();

    assert!(callers(&db, "safe_dir", "crates/a-core/tests/helpers.rs").is_empty());
}
