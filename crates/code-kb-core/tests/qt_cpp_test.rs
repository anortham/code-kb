use code_kb_core::{
    Workspace, file_skeleton_op, find_julie_extract_binary, find_references_scoped,
    find_structural_facts_scoped, open_read_only, safe_tempdir, scan_workspace,
    search_symbols_scoped,
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

    Q_PROPERTY(int index READ index WRITE setIndex NOTIFY indexChanged FINAL)
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
