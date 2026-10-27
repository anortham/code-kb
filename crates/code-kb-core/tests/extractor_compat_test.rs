use std::fs;
use std::path::PathBuf;
use std::process::Command;

use code_kb_core::db::{ensure_fts_index_path, open_read_only, open_read_write};
use code_kb_core::models::SiteLabel;
use code_kb_core::ops::{
    blast_radius_op, get_context_slice_op, get_symbol_body_op, lookup_symbols_op,
};
use code_kb_core::reference_report::{reference_report_for_impact, select_reference_target};
use code_kb_core::safe_tempdir;
use code_kb_core::sync::{create_index, delete_file, scan_workspace, update_file};
use code_kb_core::workspace::Workspace;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

#[derive(Debug, PartialEq)]
struct CssCallFact {
    identifier_id: String,
    name: String,
    containing_symbol_id: Option<String>,
    start_line: Option<i64>,
    start_column: Option<i64>,
    end_line: Option<i64>,
    end_column: Option<i64>,
    start_byte: Option<i64>,
    end_byte: Option<i64>,
    confidence: f64,
}

const CSS_SOURCE: &str = "@function --anchor_css() { result: 41; }\n@function --caller_css() { result: --anchor_css(); }\n@function --parameterized_css(--value) { result: var(--value); }\n@function --名_css() { result: 29; }\n@function --escaped\\ name_css() { result: 1; }\n@function not_css() { result: 2; }\n@function --caller_unicode() { result: --名_css(); }\n@function --caller_escaped() { result: --escaped\\ name_css(); }\n@function -- _css() { result: 30; }\n.card { --theme_css: red; color: var(--theme_css); }\n@media (min-width: 1px) { .wide { color: blue; } }\n";

fn scanned_css(source: &str) -> Repo {
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    fs::write(root.join("functions.css"), source).unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).unwrap();
    Repo {
        root,
        db,
        _temp: temp,
    }
}

fn css_call_facts(conn: &rusqlite::Connection) -> Vec<CssCallFact> {
    conn.prepare(
        "SELECT identifier_id, name, containing_symbol_id, start_line, start_column,
                end_line, end_column, start_byte, end_byte, confidence
         FROM identifiers
         WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
           AND start_line IN (2, 7, 8)
         ORDER BY start_line, start_column",
    )
    .unwrap()
    .query_map([], |row| {
        Ok(CssCallFact {
            identifier_id: row.get(0)?,
            name: row.get(1)?,
            containing_symbol_id: row.get(2)?,
            start_line: row.get(3)?,
            start_column: row.get(4)?,
            end_line: row.get(5)?,
            end_column: row.get(6)?,
            start_byte: row.get(7)?,
            end_byte: row.get(8)?,
            confidence: row.get(9)?,
        })
    })
    .unwrap()
    .collect::<rusqlite::Result<Vec<_>>>()
    .unwrap()
}

#[test]
fn css_custom_function_is_lookupable_and_body_readable() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let lookup = lookup_symbols_op(&repo.db, &conn, "--anchor_css", None, None, false, 20).unwrap();
    let declaration = lookup
        .names
        .iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .expect("lookup should expose the CSS function by its callable name");
    assert_eq!(declaration.kind, "function");
    assert_eq!(
        (
            declaration.start_line,
            declaration.start_column,
            declaration.end_line,
            declaration.end_column,
            declaration.start_byte,
            declaration.end_byte,
            declaration.body_start_byte,
            declaration.body_end_byte,
        ),
        (1, 0, 1, 40, 0, 40, Some(25), Some(40))
    );

    let (body_symbol, body) = get_symbol_body_op(
        &Workspace::new(repo.root.clone()),
        &repo.db,
        &conn,
        "--anchor_css",
        Some("functions.css"),
    )
    .unwrap();
    assert_eq!(body_symbol.symbol_id, declaration.symbol_id);
    assert_eq!(body, "@function --anchor_css() { result: 41; }");

    let parameterized = lookup_symbols_op(
        &repo.db,
        &conn,
        "--parameterized_css",
        None,
        None,
        false,
        20,
    )
    .unwrap()
    .names
    .into_iter()
    .find(|symbol| symbol.name == "--parameterized_css")
    .unwrap();
    assert_eq!(parameterized.kind, "function");

    let unicode = lookup_symbols_op(&repo.db, &conn, "--名_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--名_css")
        .unwrap();
    assert_eq!(unicode.kind, "function");
    assert_eq!(
        unicode.signature.as_deref(),
        Some("@function --名_css() { result: 29; }")
    );

    let escaped = lookup_symbols_op(
        &repo.db,
        &conn,
        "--escaped\\ name_css",
        None,
        None,
        false,
        20,
    )
    .unwrap()
    .names
    .into_iter()
    .find(|symbol| symbol.name == "--escaped\\ name_css")
    .unwrap();
    assert_eq!(escaped.kind, "function");
    assert_eq!(
        escaped.signature.as_deref(),
        Some("@function --escaped\\ name_css() { result: 1; }")
    );

    let unsupported = lookup_symbols_op(
        &repo.db,
        &conn,
        "@function not_css()",
        None,
        None,
        false,
        20,
    )
    .unwrap()
    .names
    .into_iter()
    .find(|symbol| symbol.name == "@function not_css()")
    .unwrap();
    assert_eq!(unsupported.kind, "namespace");

    let property = lookup_symbols_op(&repo.db, &conn, "--theme_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--theme_css")
        .unwrap();
    assert_eq!(property.kind, "property");
    let selector = lookup_symbols_op(&repo.db, &conn, ".card", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == ".card")
        .unwrap();
    assert_eq!(selector.kind, "property");
    let at_rule = lookup_symbols_op(&repo.db, &conn, "@media", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "@media (min-width: 1px)")
        .unwrap();
    assert_eq!(at_rule.kind, "namespace");
}

#[test]
fn unicode_and_escaped_css_call_names_match_explicit_function_tokens() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let names = conn
        .prepare(
            "SELECT name, start_line FROM identifiers
             WHERE path = 'functions.css' AND kind = 'call' AND start_line IN (7, 8)
             ORDER BY start_line, start_column",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        names,
        vec![
            ("--名_css".to_string(), 7),
            ("--escaped\\ name_css".to_string(), 8)
        ]
    );
}

#[test]
fn pinned_extractor_keeps_unicode_separator_declaration_names_distinct_from_source() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let declaration = conn
        .query_row(
            "SELECT name, signature FROM symbols
             WHERE path = 'functions.css' AND start_line = 9 AND kind = 'namespace'",
            [],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .unwrap();
    assert_eq!(declaration.0, "@function -- _css()");
    assert_eq!(declaration.1, "@function -- _css() { result: 30; }");
}

#[test]
fn css_custom_function_call_is_reported_as_a_reference() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let call_name: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND kind = 'call' AND start_line = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(call_name, "--anchor_css");
    let target = select_reference_target(&conn, "--anchor_css", None).unwrap();
    let report = reference_report_for_impact(&conn, &repo.root, &target).unwrap();
    let call = report
        .sites
        .iter()
        .find(|site| site.site.path == "functions.css" && site.site.start_line == Some(2))
        .expect("the extracted CSS function call should reach its declaration");
    assert_eq!(call.label, SiteLabel::Proven);
}

#[test]
fn unicode_and_escaped_css_calls_reach_context_references_and_impact() {
    let repo = scanned_css(CSS_SOURCE);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();

    for (target_name, caller_name, line) in [
        ("--名_css", "--caller_unicode", 7),
        ("--escaped\\ name_css", "--caller_escaped", 8),
    ] {
        let call_names = conn
            .prepare(
                "SELECT name FROM identifiers WHERE path = 'functions.css' AND kind = 'call'
                   AND containing_symbol_id = (SELECT symbol_id FROM symbols
                     WHERE path = 'functions.css' AND name = ?1 AND kind = 'function')
                 ORDER BY start_line, start_column",
            )
            .unwrap()
            .query_map([caller_name], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(call_names, vec![target_name]);

        let context = get_context_slice_op(
            &workspace,
            &repo.db,
            &conn,
            caller_name,
            Some("functions.css"),
            false,
        )
        .unwrap();
        assert!(
            context
                .callee_signatures
                .iter()
                .any(|signature| signature.contains(target_name))
        );

        let target = select_reference_target(&conn, target_name, None).unwrap();
        let report = reference_report_for_impact(&conn, &repo.root, &target).unwrap();
        assert!(report.sites.iter().any(|site| {
            site.label == SiteLabel::Proven
                && site.site.from_symbol_name == caller_name
                && site.site.start_line == Some(line)
        }));

        let impact = blast_radius_op(&workspace, &conn, Some(target_name), None, 2, 20).unwrap();
        assert!(
            impact
                .impacted_symbols
                .iter()
                .any(|symbol| symbol.name == caller_name && symbol.path == "functions.css")
        );
    }
}

#[test]
fn missing_css_unicode_call_fact_does_not_create_a_false_reference() {
    let repo = scanned_css(
        "@function --foo() { result: 1; }\n\
         @function --foo名() { result: 2; }\n\
         @function --caller_css() { result: --foo名(); }\n",
    );
    let conn = open_read_only(&repo.db).unwrap();
    let call_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM identifiers
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
               AND start_line = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let shorter_target = select_reference_target(&conn, "--foo", None).unwrap();
    let report = reference_report_for_impact(&conn, &repo.root, &shorter_target).unwrap();
    assert_eq!(call_count, 0);
    assert!(!report.sites.iter().any(|site| {
        site.label == SiteLabel::Proven && site.site.from_symbol_name == "--caller_css"
    }));
}

#[test]
fn css_call_normalization_rejects_identifier_spans_that_miss_the_source_token() {
    let repo = scanned_css(CSS_SOURCE);
    let writable = open_read_write(&repo.db).unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = '--', start_byte = start_byte + 1,
                   end_byte = end_byte + 1
                 WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
                [],
            )
            .unwrap(),
        1
    );
    drop(writable);

    ensure_fts_index_path(&repo.db).unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "--");
}

#[test]
fn css_call_normalization_leaves_ambiguous_function_targets_unchanged() {
    let repo = scanned_css(
        "@function --名_css() { result: 1; }\n\
         @function --名_css() { result: 2; }\n\
         @function --caller() { result: --名_css(); }\n",
    );
    let conn = open_read_only(&repo.db).unwrap();
    let declarations: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'function'
               AND name = '--名_css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let call: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
               AND start_line = 3",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(declarations, 2);
    assert_eq!(call, "--");
}

#[test]
fn css_call_normalization_leaves_malformed_facts_when_source_is_missing() {
    let repo = scanned_css(CSS_SOURCE);
    let writable = open_read_write(&repo.db).unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = '--'
                 WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
                [],
            )
            .unwrap(),
        1
    );
    drop(writable);
    fs::remove_file(repo.root.join("functions.css")).unwrap();

    ensure_fts_index_path(&repo.db).unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "--");
}

#[test]
fn large_css_function_bodies_keep_name_and_call_normalization() {
    let large_value = "x".repeat(128 * 1024);
    let source = format!(
        "@function --large_css() {{ result: {large_value}; }}\n\
         @function --caller_large_css() {{ result: --large_css(); }}\n"
    );
    let repo = scanned_css(&source);
    let conn = open_read_only(&repo.db).unwrap();
    let declaration: (String, i64) = conn
        .query_row(
            "SELECT kind, length(signature) FROM symbols
             WHERE path = 'functions.css' AND name = '--large_css'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    let call_name: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
               AND start_line = 2",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(declaration.0, "function");
    assert!(declaration.1 > 128 * 1024);
    assert_eq!(call_name, "--large_css");
}

#[cfg(unix)]
#[test]
fn css_call_normalization_does_not_read_a_source_symlink_outside_the_workspace() {
    let repo = scanned_css(CSS_SOURCE);
    let external = tempfile::tempdir().unwrap();
    let external_source = external.path().join("functions.css");
    fs::write(&external_source, CSS_SOURCE).unwrap();
    fs::remove_file(repo.root.join("functions.css")).unwrap();
    std::os::unix::fs::symlink(&external_source, repo.root.join("functions.css")).unwrap();

    let writable = open_read_write(&repo.db).unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = '--'
                 WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
                [],
            )
            .unwrap(),
        1
    );
    drop(writable);

    ensure_fts_index_path(&repo.db).unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let name: String = conn
        .query_row(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND kind = 'call' AND start_line = 7",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(name, "--");
}

#[test]
fn existing_css_function_index_normalizes_repeatably_without_changing_identity_or_spans() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let before_calls = css_call_facts(&conn);
    let before = lookup_symbols_op(&repo.db, &conn, "--anchor_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .unwrap();
    let expected = (
        before.symbol_id.clone(),
        before.signature.clone(),
        before.start_line,
        before.start_column,
        before.end_line,
        before.end_column,
        before.start_byte,
        before.end_byte,
        before.body_start_byte,
        before.body_end_byte,
    );
    drop(conn);

    let writable = open_read_write(&repo.db).unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE symbols SET name = '@function --anchor_css()', kind = 'namespace' WHERE symbol_id = ?1",
                [&before.symbol_id],
            )
            .unwrap(),
        1
    );
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = CASE start_line
                     WHEN 7 THEN '--' WHEN 8 THEN 'name_css' END
                 WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
                   AND start_line IN (7, 8)",
                [],
            )
            .unwrap(),
        2
    );
    drop(writable);

    ensure_fts_index_path(&repo.db).unwrap();
    ensure_fts_index_path(&repo.db).unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let after_calls = css_call_facts(&conn);
    assert_eq!(after_calls, before_calls);
    assert_eq!(
        after_calls
            .iter()
            .map(|fact| fact.name.as_str())
            .collect::<Vec<_>>(),
        vec!["--anchor_css", "--名_css", "--escaped\\ name_css"]
    );
    let after = lookup_symbols_op(&repo.db, &conn, "--anchor_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .unwrap();
    assert_eq!(
        (
            after.symbol_id,
            after.signature,
            after.start_line,
            after.start_column,
            after.end_line,
            after.end_column,
            after.start_byte,
            after.end_byte,
            after.body_start_byte,
            after.body_end_byte,
        ),
        expected
    );
    assert!(
        lookup_symbols_op(
            &repo.db,
            &conn,
            "@function --anchor_css()",
            None,
            None,
            false,
            20
        )
        .unwrap()
        .names
        .is_empty()
    );
}

#[test]
fn incremental_css_update_normalizes_function_renames_without_stale_names() {
    let repo = scanned_css(CSS_SOURCE);
    fs::write(
        repo.root.join("functions.css"),
        "@function --replacement_css() { result: 43; }\n\
         @function --caller_css() { result: --replacement_css(); }\n\
         @function --名_updated_css() { result: 44; }\n\
         @function --caller_unicode_updated_css() { result: --名_updated_css(); }\n\
         @function --escaped\\ updated_css() { result: 45; }\n\
         @function --caller_escaped_updated_css() { result: --escaped\\ updated_css(); }\n",
    )
    .unwrap();
    update_file(
        &Workspace::new(repo.root.clone()),
        &repo.db,
        "functions.css",
    )
    .unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let renamed = lookup_symbols_op(&repo.db, &conn, "--replacement_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--replacement_css")
        .unwrap();
    assert_eq!(renamed.kind, "function");
    assert!(
        renamed
            .signature
            .unwrap()
            .starts_with("@function --replacement_css()")
    );
    let calls: Vec<String> = conn
        .prepare(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
               AND start_line IN (2, 4, 6)
             ORDER BY start_line, start_column",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        calls,
        vec![
            "--replacement_css",
            "--名_updated_css",
            "--escaped\\ updated_css"
        ]
    );
    let context = get_context_slice_op(
        &Workspace::new(repo.root.clone()),
        &repo.db,
        &conn,
        "--caller_unicode_updated_css",
        Some("functions.css"),
        false,
    )
    .unwrap();
    assert!(
        context
            .callee_signatures
            .iter()
            .any(|signature| signature.contains("--名_updated_css"))
    );
    assert!(
        lookup_symbols_op(&repo.db, &conn, "--anchor_css", None, None, false, 20)
            .unwrap()
            .names
            .is_empty()
    );
    assert!(
        lookup_symbols_op(
            &repo.db,
            &conn,
            "@function --replacement_css()",
            None,
            None,
            false,
            20
        )
        .unwrap()
        .names
        .is_empty()
    );
}

#[test]
fn incremental_css_delete_normalizes_remaining_function_facts() {
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    fs::write(root.join("functions.css"), CSS_SOURCE).unwrap();
    fs::write(root.join("delete.css"), ".temporary { color: red; }\n").unwrap();
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).unwrap();

    let conn = open_read_only(&db).unwrap();
    let symbol_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE path = 'functions.css' AND name = '--anchor_css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);

    let writable = open_read_write(&db).unwrap();
    writable
        .execute(
            "UPDATE symbols SET name = '@function --anchor_css()', kind = 'namespace'
             WHERE symbol_id = ?1",
            [&symbol_id],
        )
        .unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = CASE start_line
                     WHEN 7 THEN '--' WHEN 8 THEN 'name_css' END
                 WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
                   AND start_line IN (7, 8)",
                [],
            )
            .unwrap(),
        2
    );
    drop(writable);

    delete_file(&workspace, &db, "delete.css").unwrap();

    let conn = open_read_only(&db).unwrap();
    let declaration = lookup_symbols_op(&db, &conn, "--anchor_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .unwrap();
    assert_eq!(declaration.symbol_id, symbol_id);
    let remaining_calls = css_call_facts(&conn);
    assert_eq!(
        remaining_calls
            .iter()
            .map(|fact| fact.name.as_str())
            .collect::<Vec<_>>(),
        vec!["--anchor_css", "--名_css", "--escaped\\ name_css"]
    );
    let deleted_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE path = 'delete.css'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(deleted_rows, 0);
}

#[test]
fn copied_worktree_index_normalizes_existing_css_function_facts() {
    let repo = scanned_css(CSS_SOURCE);
    let conn = open_read_only(&repo.db).unwrap();
    let original = lookup_symbols_op(&repo.db, &conn, "--anchor_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .unwrap();
    drop(conn);

    let run_git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&repo.root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run_git(&["init", "-b", "main"]);
    run_git(&["config", "user.name", "Test User"]);
    run_git(&["config", "user.email", "test@example.com"]);
    run_git(&["add", "functions.css"]);
    run_git(&["commit", "-m", "CSS function fixture"]);

    let writable = open_read_write(&repo.db).unwrap();
    assert_eq!(
        writable
            .execute(
                "UPDATE symbols SET name = '@function --anchor_css()', kind = 'namespace' WHERE symbol_id = ?1",
                [&original.symbol_id],
            )
            .unwrap(),
        1
    );
    assert_eq!(
        writable
            .execute(
                "UPDATE identifiers SET name = CASE start_line
                     WHEN 7 THEN '--' WHEN 8 THEN 'name_css' END
                 WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
                   AND start_line IN (7, 8)",
                [],
            )
            .unwrap(),
        2
    );
    drop(writable);

    let worktree_root = repo.root.join(".worktrees").join("css-copy");
    let worktree_path = worktree_root.to_str().unwrap();
    run_git(&["worktree", "add", worktree_path, "-b", "css-copy"]);
    let worktree = Workspace::new(worktree_root.clone());
    let worktree_db = worktree_root.join(".code-kb").join("artifact.db");
    create_index(&worktree, &worktree_db).unwrap();

    let conn = open_read_only(&worktree_db).unwrap();
    let copied = lookup_symbols_op(&worktree_db, &conn, "--anchor_css", None, None, false, 20)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == "--anchor_css")
        .unwrap();
    assert_eq!(copied.kind, "function");
    assert_eq!(copied.symbol_id, original.symbol_id);
    let call_names: Vec<String> = conn
        .prepare(
            "SELECT name FROM identifiers
             WHERE path = 'functions.css' AND language = 'css' AND kind = 'call'
               AND start_line IN (2, 7, 8)
             ORDER BY start_line, start_column",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(
        call_names,
        vec!["--anchor_css", "--名_css", "--escaped\\ name_css"]
    );
}
