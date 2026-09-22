use code_kb_core::{
    Workspace, file_skeleton_op, find_julie_extract_binary, find_references_scoped,
    find_structural_facts_scoped, fts_search_symbols_scoped, get_file, get_symbol_body_op,
    open_read_only, safe_tempdir, scan_workspace, search_symbols_scoped,
};
use std::fs;
use std::path::{Path, PathBuf};

const COLUMNVIEW_H: &str = r#"// SPDX-FileCopyrightText: 2026 The Kirigami Authors
// SPDX-License-Identifier: LGPL-2.0-or-later

#pragma once

#include <QObject>
#include <QPointF>

class ColumnView;

class KIRIGAMI2_EXPORT ColumnViewAttached : public QObject
{
    Q_OBJECT
    QML_ELEMENT
    QML_ATTACHED(ColumnViewAttached)

    Q_PROPERTY(int index READ index WRITE setIndex NOTIFY indexChanged DESIGNABLE false SCRIPTABLE true STORED false USER true REVISION 2 FINAL)
    Q_PROPERTY(ColumnView *view READ view NOTIFY viewChanged FINAL)
    Q_PROPERTY(QPointF origin
               MEMBER origin
               CONSTANT FINAL)

public:
    enum class ColumnResizeMode {
        FixedColumns,
        DynamicColumns,
    };
    Q_ENUM(ColumnResizeMode)

    explicit ColumnViewAttached(QObject *parent = nullptr);
    ~ColumnViewAttached() override;

    int index() const;
    void setIndex(int index);

    QQuickItem *contentItem() const;
    const QString &name() const;
    QList<int> *items();
    static ColumnViewAttached *instance();

    Q_INVOKABLE void reset();

public Q_SLOTS:
    void refresh();

Q_SIGNALS:
    void indexChanged();
    void viewChanged(ColumnView *view);

private:
    int m_index = 0;
    QObject *m_parent = Q_NULLPTR;
};

class KIRIGAMI2_EXPORT ScrollIntentionEvent : public QObject
{
    Q_OBJECT
    Q_PROPERTY(QPointF delta MEMBER delta CONSTANT FINAL)

public:
    QPointF delta;
};

struct Geometry {
    int width;
};
"#;

const HEADER_PATH: &str = "src/layouts/columnview.h";

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
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

fn scanned_header() -> (tempfile::TempDir, PathBuf) {
    scanned_repo(&[(HEADER_PATH, COLUMNVIEW_H)])
}

fn skeleton(repo: &tempfile::TempDir, db_path: &Path) -> String {
    let workspace = Workspace::new(repo.path().to_path_buf());
    let conn = open_read_only(db_path).unwrap();
    file_skeleton_op(&workspace, db_path, &conn, HEADER_PATH).unwrap()
}

#[test]
fn a_qt_header_skeleton_lists_every_q_property_under_its_class() {
    let (repo, db_path) = scanned_header();

    let out = skeleton(&repo, &db_path);

    for name in ["index", "view", "origin", "delta"] {
        let row = out
            .lines()
            .find(|l| l.contains("Q_PROPERTY(") && l.contains(name))
            .unwrap_or_else(|| panic!("no Q_PROPERTY row for {name} in:\n{out}"));
        assert!(row.starts_with("    "), "{row}");
    }
    assert_eq!(out.matches("Q_PROPERTY(").count(), 4, "{out}");
}

#[test]
fn a_qt_header_skeleton_lists_a_signal_as_an_event_row() {
    let (repo, db_path) = scanned_header();
    let conn = open_read_only(&db_path).unwrap();

    let out = skeleton(&repo, &db_path);

    for name in ["indexChanged", "viewChanged"] {
        let hit = search_symbols_scoped(&conn, name, None, Some(HEADER_PATH), false, 10)
            .unwrap()
            .into_iter()
            .find(|s| s.kind == "event")
            .unwrap_or_else(|| panic!("no event symbol for {name}"));
        assert!(hit.parent_symbol_id.is_some(), "{name}");
        let row = out
            .lines()
            .find(|l| l.contains(&format!("{name}(")))
            .unwrap_or_else(|| panic!("no skeleton row for {name} in:\n{out}"));
        assert!(row.contains("// event L"), "{row}");
    }
}

#[test]
fn a_qt_header_skeleton_reports_no_parse_error() {
    let (repo, db_path) = scanned_header();

    let out = skeleton(&repo, &db_path);

    assert!(!out.contains("parse error"), "{out}");
}

#[test]
fn a_qt_header_skeleton_omits_the_forward_declaration() {
    let (repo, db_path) = scanned_header();

    let out = skeleton(&repo, &db_path);

    let class_rows: Vec<&str> = out.lines().filter(|l| l.starts_with("class ")).collect();

    assert_eq!(
        class_rows,
        vec![
            "class ColumnViewAttached : public QObject {",
            "class ScrollIntentionEvent : public QObject {"
        ],
        "{out}"
    );
}

#[test]
fn lookup_of_a_forward_declared_class_returns_one_definition_row() {
    let (_repo, db_path) = scanned_repo(&[
        (HEADER_PATH, COLUMNVIEW_H),
        (
            "src/layouts/columnview2.h",
            "#pragma once\n\nclass ColumnView : public QObject\n{\n    Q_OBJECT\n\npublic:\n    int count() const;\n};\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let hits = search_symbols_scoped(&conn, "ColumnView", None, Some("src/layouts"), false, 20)
        .unwrap()
        .into_iter()
        .filter(|s| s.name == "ColumnView")
        .collect::<Vec<_>>();

    assert_eq!(hits.len(), 1, "{hits:#?}");
    assert_eq!(hits[0].kind, "class");
    assert_eq!(hits[0].path, "src/layouts/columnview2.h");
    assert_eq!(hits[0].start_line, 3);
}

#[test]
fn lookup_of_a_property_name_returns_its_q_property_signature() {
    let (_repo, db_path) = scanned_header();
    let conn = open_read_only(&db_path).unwrap();

    let hit = search_symbols_scoped(&conn, "origin", None, Some(HEADER_PATH), false, 10)
        .unwrap()
        .into_iter()
        .find(|s| s.kind == "property")
        .unwrap_or_else(|| panic!("no property symbol named origin"));

    let signature = hit.signature.expect("property row carries a signature");
    assert!(signature.starts_with("Q_PROPERTY("), "{signature}");
    assert!(signature.contains("MEMBER origin"), "{signature}");
}

#[test]
fn the_property_alias_returns_one_scoped_fact_per_q_property() {
    let (_repo, db_path) = scanned_header();
    let conn = open_read_only(&db_path).unwrap();

    let facts = find_structural_facts_scoped(&conn, "property", Some("src/layouts"), 50).unwrap();

    assert_eq!(facts.len(), 4, "{facts:#?}");
    assert!(
        facts
            .iter()
            .all(|f| f.pattern_id == "cpp.qt_property.v1" && f.path == HEADER_PATH),
        "{facts:#?}"
    );
    let owners: Vec<&str> = facts
        .iter()
        .map(|f| f.containing_symbol_name.as_deref().unwrap_or("<none>"))
        .collect();
    assert_eq!(
        owners,
        vec![
            "ColumnViewAttached",
            "ColumnViewAttached",
            "ColumnViewAttached",
            "ScrollIntentionEvent"
        ],
        "{facts:#?}"
    );

    let index = facts
        .iter()
        .find(|fact| fact.key.as_deref() == Some("index"))
        .expect("index Q_PROPERTY fact");
    let metadata = index.metadata.as_ref().expect("property metadata");
    for (key, expected) in [
        ("designable", "false"),
        ("scriptable", "true"),
        ("stored", "false"),
        ("user", "true"),
        ("revision", "2"),
    ] {
        assert_eq!(
            metadata.get(key).and_then(|value| value.as_str()),
            Some(expected)
        );
    }
    let delta = facts
        .iter()
        .find(|fact| fact.key.as_deref() == Some("delta"))
        .expect("delta Q_PROPERTY fact");
    let delta_metadata = delta.metadata.as_ref().expect("property metadata");
    for key in ["designable", "scriptable", "stored", "user", "revision"] {
        assert!(delta_metadata.get(key).is_none(), "{delta_metadata}");
    }
}

#[test]
fn qt_property_facts_render_their_agent_useful_metadata() {
    let (_repo, db_path) = scanned_header();
    let conn = open_read_only(&db_path).unwrap();

    let facts = find_structural_facts_scoped(&conn, "property", Some(HEADER_PATH), 50).unwrap();
    let output = code_kb_core::format_structural_facts(&facts, &[], "property", 50);

    assert!(output.contains("property_type: int"), "{output}");
    for detail in [
        "designable: false",
        "scriptable: true",
        "stored: false",
        "user: true",
        "revision: 2",
    ] {
        assert!(output.contains(detail), "{output}");
    }
}

#[test]
fn qt_header_native_edits_refresh_qt_macros() {
    let (repo, db_path) = scanned_header();
    let workspace = Workspace::new(repo.path().to_path_buf());
    let original_hash = {
        let conn = open_read_only(&db_path).unwrap();
        get_file(&conn, HEADER_PATH).unwrap().unwrap().content_hash
    };
    let locked = repo.path().join("src/locked.rs");
    fs::write(&locked, "pub fn locked_symbol() {}\n").unwrap();
    scan_workspace(&workspace, &db_path, true).unwrap();
    let header = repo.path().join(HEADER_PATH);
    fs::write(&header, COLUMNVIEW_H.replace("m_index = 0", "m_index = 1")).unwrap();
    assert!(
        fs::read_to_string(repo.path().join(HEADER_PATH))
            .unwrap()
            .contains("m_index = 1")
    );
    #[cfg(unix)]
    fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o000)).unwrap();
    let _ = skeleton(&repo, &db_path);
    #[cfg(unix)]
    fs::set_permissions(&locked, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    assert_ne!(
        get_file(&conn, HEADER_PATH).unwrap().unwrap().content_hash,
        original_hash
    );
    assert!(
        search_symbols_scoped(&conn, "index", None, Some(HEADER_PATH), false, 10)
            .unwrap()
            .iter()
            .any(|symbol| symbol.kind == "property"),
        "edited Qt header lost its Q_PROPERTY symbols"
    );
    assert!(
        fts_search_symbols_scoped(
            &conn,
            "ColumnViewAttached",
            None,
            Some(HEADER_PATH),
            false,
            10
        )
        .unwrap()
        .iter()
        .any(|symbol| symbol.symbol.kind == "class"),
        "edited Qt header was not available through FTS search"
    );
}

#[test]
fn javascript_qml_directives_exclude_trailing_comments_from_agent_facing_spans() {
    let source = ".pragma library // helper module\n.import QtQml 2.15 as Qml // namespace\nfunction value() { return Qml; }\n";
    let (repo, db_path) = scanned_repo(&[("src/helpers.js", source)]);
    let workspace = Workspace::new(repo.path().to_path_buf());
    let conn = open_read_only(&db_path).unwrap();

    let import = search_symbols_scoped(&conn, "QtQml", None, Some("src/helpers.js"), false, 10)
        .unwrap()
        .into_iter()
        .find(|symbol| symbol.kind == "import")
        .expect("QML directive import");
    assert_eq!(
        import.signature.as_deref(),
        Some(".import QtQml 2.15 as Qml")
    );
    assert_eq!(import.end_column, 25);
    let (_, body) =
        get_symbol_body_op(&workspace, &db_path, &conn, "QtQml", Some("src/helpers.js"))
            .expect("QML directive import body");
    assert_eq!(body, ".import QtQml 2.15 as Qml");

    let facts = find_structural_facts_scoped(&conn, "pragma", Some("src/helpers.js"), 10).unwrap();
    assert_eq!(facts.len(), 1, "{facts:#?}");
    assert_eq!(facts[0].pattern_id, "javascript.qml_directive.v1");
    assert_eq!(facts[0].start_line, 1);
    assert_eq!(facts[0].end_line, 1);
}

#[test]
fn a_signal_declaration_is_not_reported_as_a_handler_candidate() {
    let (_repo, db_path) = scanned_header();
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "indexChanged",
        "callers",
        50,
        false,
        Some(HEADER_PATH),
    )
    .unwrap();

    assert!(
        refs.iter().all(|r| !r.kind.starts_with("handler")),
        "{refs:#?}"
    );
}
