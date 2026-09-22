use code_kb_core::{
    Workspace, compute_blast_radius_scoped, find_callee_signatures, find_julie_extract_binary,
    find_references_for_symbol, find_references_scoped, open_read_only, safe_tempdir,
    scan_workspace,
};
use std::fs;
use std::path::PathBuf;

const POSITIVE_QML: &str = r#"
import QtQuick 2.15

Item { component Detail: Item { function helper() {} Component.onCompleted: helper() } }
"#;

const SCOPE_QML: &str = r#"
import QtQuick 2.15

Item {
    id: root
    property int gap: 100
    function helper() {}

    function parameterShadow(root) { root.helper() }
    function localShadow() { let root = {}; root.helper() }

    Column {
        property int gap: 50
        Rectangle { height: parent.gap }
        function ownGap() { return this.gap }
    }

    component First: Item {
        function run() { secondOnly() }
        Component.onCompleted: secondOnly()
    }
    component Second: Item { function secondOnly() {} }
}
"#;

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let repo = safe_tempdir();
    for (path, content) in files {
        let file = repo.path().join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, content).unwrap();
    }
    let db = repo.path().join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(repo.path().to_path_buf()), &db, true).unwrap();
    (repo, db)
}

#[test]
fn same_line_inline_handler_calls_its_own_component_helper() {
    let (_repo, db) = scanned_repo(&[("src/Inline.qml", POSITIVE_QML)]);
    let conn = open_read_only(&db).unwrap();

    let detail_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE path = 'src/Inline.qml' AND name = 'Detail' AND parent_symbol_id IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let helper_parent: String = conn
        .query_row(
            "SELECT parent_symbol_id FROM symbols WHERE path = 'src/Inline.qml' AND name = 'helper'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs = find_references_scoped(
        &conn,
        "helper",
        "callers",
        10,
        false,
        Some("src/Inline.qml"),
    )
    .unwrap();

    assert_eq!(refs.len(), 1, "{refs:#?}");
    assert_eq!(refs[0].from_symbol_id, detail_id);
    assert_eq!(helper_parent, refs[0].from_symbol_id);
    assert_eq!(refs[0].kind, "calls");
    let inline_extends: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pending_relationships WHERE from_symbol_id = ?1 AND target_terminal_name = 'Item' AND kind = 'extends'",
            [&refs[0].from_symbol_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inline_extends, 1);
}

#[test]
fn shadowed_and_sibling_qml_calls_stay_unresolved() {
    let (_repo, db) = scanned_repo(&[("src/Scope.qml", SCOPE_QML)]);
    let conn = open_read_only(&db).unwrap();

    for name in ["helper", "secondOnly"] {
        let refs = find_references_scoped(&conn, name, "callers", 10, false, Some("src/Scope.qml"))
            .unwrap();
        assert!(refs.is_empty(), "{name}: {refs:#?}");
    }

    let parameter_shadow_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE path = 'src/Scope.qml' AND name = 'parameterShadow'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let signatures =
        find_callee_signatures(&conn, "parameterShadow", &parameter_shadow_id, 10, false).unwrap();
    assert!(signatures.is_empty(), "{signatures:#?}");
    let external_signatures =
        find_callee_signatures(&conn, "parameterShadow", &parameter_shadow_id, 10, true).unwrap();
    assert!(
        external_signatures
            .iter()
            .any(|signature| signature.starts_with("root.helper ")),
        "{external_signatures:#?}"
    );

    let impact =
        compute_blast_radius_scoped(&conn, &["helper"], Some("src/Scope.qml"), &[], 2, 10).unwrap();
    assert!(impact.impacted_symbols.is_empty(), "{impact:#?}");
}

#[test]
fn nested_parent_and_this_bindings_use_the_nested_object_property() {
    let (_repo, db) = scanned_repo(&[("src/Scope.qml", SCOPE_QML)]);
    let conn = open_read_only(&db).unwrap();
    let gap_id: String = conn
        .query_row(
            "SELECT property.symbol_id
             FROM symbols property
             JOIN symbols owner ON owner.symbol_id = property.parent_symbol_id
             WHERE property.path = 'src/Scope.qml' AND property.name = 'gap' AND owner.name = 'Column'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs = find_references_for_symbol(&conn, "gap", "callers", 10, &gap_id).unwrap();

    assert_eq!(refs.len(), 2, "{refs:#?}");
    assert!(refs.iter().all(|reference| reference.kind == "uses"));
}
