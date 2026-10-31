use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use code_kb_core::{
    db::open_read_only,
    models::ReferenceReport,
    ops::blast_radius_op,
    reference_report::{reference_report, select_reference_target},
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use rusqlite::Connection;

const FILES: [(&str, &str); 3] = [
    ("app/core.py", "def target():\n    return 1\n"),
    (
        "app/service.py",
        "from app.core import target\n\n\ndef caller():\n    return target()\n",
    ),
    (
        "tests/test_service.py",
        "from app.service import caller\n\n\ndef test_caller():\n    assert caller() == 1\n",
    ),
];

fn scanned_repo() -> (tempfile::TempDir, Workspace, PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in FILES {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    (temp, workspace, db)
}

fn delete_callers_once_the_next_answer_starts(conn: &Connection, db: &Path) -> Arc<AtomicBool> {
    let deleted = Arc::new(AtomicBool::new(false));
    let flag = deleted.clone();
    let db = db.to_path_buf();
    conn.progress_handler(
        100,
        Some(move || {
            if !flag.swap(true, Ordering::SeqCst) {
                Connection::open(&db)
                    .unwrap()
                    .execute(
                        "DELETE FROM symbols WHERE path IN ('app/service.py', 'tests/test_service.py')",
                        [],
                    )
                    .unwrap();
            }
            false
        }),
    )
    .unwrap();
    deleted
}

#[test]
fn blast_radius_answers_from_one_snapshot_while_the_index_changes() {
    let (_temp, workspace, db) = scanned_repo();
    let conn = open_read_only(&db).unwrap();
    let before = blast_radius_op(&workspace, &conn, Some("target"), None, 2, 20).unwrap();
    assert!(
        before
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == "caller"),
        "{before:?}"
    );

    let deleted = delete_callers_once_the_next_answer_starts(&conn, &db);
    let during = blast_radius_op(&workspace, &conn, Some("target"), None, 2, 20).unwrap();

    assert!(deleted.load(Ordering::SeqCst));
    assert_eq!(during, before);
}

#[test]
fn a_callers_report_answers_from_one_snapshot_while_the_index_changes() {
    let (_temp, workspace, db) = scanned_repo();
    let conn = open_read_only(&db).unwrap();
    let target = select_reference_target(&conn, "target", None).unwrap();
    let before = reference_report(&conn, &workspace.root, &target, false).unwrap();
    assert!(
        before
            .sites
            .iter()
            .any(|site| site.site.path == "app/service.py"),
        "{before:?}"
    );

    let deleted = delete_callers_once_the_next_answer_starts(&conn, &db);
    let during = reference_report(&conn, &workspace.root, &target, false).unwrap();

    assert!(deleted.load(Ordering::SeqCst));
    assert_eq!(without_timing(&during), without_timing(&before));
}

fn without_timing(report: &ReferenceReport) -> serde_json::Value {
    let mut value = serde_json::to_value(report).unwrap();
    value["text_scan"]["elapsed_ms"] = serde_json::Value::Null;
    value
}
