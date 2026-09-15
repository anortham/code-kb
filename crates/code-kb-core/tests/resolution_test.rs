use code_kb_core::{
    Workspace, compute_blast_radius_scoped, find_callee_signatures, find_julie_extract_binary,
    find_references_scoped, get_symbol_by_name, open_read_only, safe_tempdir, scan_workspace,
};
use std::fs;

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root), &db_path, true).expect("scan failed");
    (temp_dir, db_path)
}

const SAME_NAME_ACROSS_DIRECTORIES: &[(&str, &str)] = &[
    ("pkg/net/run.py", "def run():\n    return 1\n"),
    (
        "pkg/net/client.py",
        "from .run import run\n\ndef client():\n    return run()\n",
    ),
    ("pkg/db/run.py", "def run():\n    return 2\n"),
    (
        "pkg/db/job.py",
        "from .run import run\n\ndef job():\n    return run()\n",
    ),
    ("pkg/db/loose.py", "def loose():\n    return run()\n"),
];

const RECEIVER_TYPES: &[(&str, &str)] = &[(
    "src/a.rs",
    "pub struct A;\nimpl A {\n    pub fn go(&self) -> u8 { 1 }\n}\npub struct B;\nimpl B {\n    pub fn go(&self) -> u8 { 2 }\n}\npub fn use_both(a: &A, b: &B) -> u8 {\n    a.go() + b.go()\n}\n",
)];

fn caller_names(refs: &[code_kb_core::ReferenceSite]) -> Vec<String> {
    let mut names: Vec<String> = refs.iter().map(|r| r.from_symbol_name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn callers_prefer_the_candidate_in_the_same_directory() {
    let (_repo, db) = scanned_repo(SAME_NAME_ACROSS_DIRECTORIES);
    let conn = open_read_only(&db).unwrap();

    let net =
        find_references_scoped(&conn, "run", "callers", 20, false, Some("pkg/net/run.py")).unwrap();
    let db_ =
        find_references_scoped(&conn, "run", "callers", 20, false, Some("pkg/db/run.py")).unwrap();

    assert_eq!(caller_names(&net), ["client"]);
    assert_eq!(caller_names(&db_), ["job", "loose"]);
}

#[test]
fn callee_signatures_prefer_the_candidate_in_the_same_directory() {
    let (_repo, db) = scanned_repo(SAME_NAME_ACROSS_DIRECTORIES);
    let conn = open_read_only(&db).unwrap();
    let client = get_symbol_by_name(&conn, "client", Some("pkg/net/client.py"))
        .unwrap()
        .unwrap();

    let sigs = find_callee_signatures(&conn, "client", &client.symbol_id, 10, false).unwrap();

    assert!(
        sigs.iter().any(|s| s.contains("pkg/net/run.py")),
        "{sigs:?}"
    );
    assert!(
        !sigs.iter().any(|s| s.contains("pkg/db/run.py")),
        "{sigs:?}"
    );
}

#[test]
fn blast_radius_prefers_the_candidate_in_the_same_directory() {
    let (_repo, db) = scanned_repo(SAME_NAME_ACROSS_DIRECTORIES);
    let conn = open_read_only(&db).unwrap();

    let radius =
        compute_blast_radius_scoped(&conn, &["run"], Some("pkg/net/run.py"), &[], 3, 50).unwrap();

    let mut impacted: Vec<&str> = radius
        .impacted_symbols
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    impacted.sort();
    assert_eq!(impacted, ["client"]);
}

#[test]
fn callers_use_the_receiver_variable_type_to_pick_the_method() {
    let (_repo, db) = scanned_repo(RECEIVER_TYPES);
    let conn = open_read_only(&db).unwrap();

    let a_go = find_references_scoped(&conn, "A::go", "callers", 20, false, None).unwrap();
    let b_go = find_references_scoped(&conn, "B::go", "callers", 20, false, None).unwrap();

    assert_eq!(a_go.len(), 1, "{a_go:?}");
    assert_eq!(a_go[0].from_symbol_name, "use_both");
    assert_eq!(a_go[0].start_line, Some(10));
    assert_eq!(b_go.len(), 1, "{b_go:?}");
    assert_eq!(b_go[0].from_symbol_name, "use_both");
}
