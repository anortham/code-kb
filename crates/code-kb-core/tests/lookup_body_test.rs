use code_kb_core::{
    OUTPUT_BUDGET_CHARS, SymbolLookup, Workspace, find_julie_extract_binary, format_symbol_bodies,
    get_symbol_body_op, lookup_symbols_with_body_op, open_read_only, render_symbol_lookup,
    safe_tempdir, scan_workspace,
};
use std::fs;
use std::path::PathBuf;

struct Repo {
    _dir: tempfile::TempDir,
    workspace: Workspace,
    db: PathBuf,
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let dir = safe_tempdir();
    let root = dir.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    Repo {
        _dir: dir,
        workspace,
        db,
    }
}

fn lookup(repo: &Repo, query: &str, limit: usize) -> (SymbolLookup, String) {
    let conn = open_read_only(&repo.db).unwrap();
    let found = lookup_symbols_with_body_op(
        &repo.workspace,
        &repo.db,
        &conn,
        query,
        None,
        None,
        false,
        limit,
    )
    .unwrap();
    let text = render_symbol_lookup(&conn, query, &found, limit).unwrap();
    (found, text)
}

fn body_text(repo: &Repo, name: &str) -> String {
    let conn = open_read_only(&repo.db).unwrap();
    let body = get_symbol_body_op(&repo.workspace, &repo.db, &conn, name, None).unwrap();
    format_symbol_bodies(&[body])
}

const NET: &str = "pub fn retry_count() -> u32 {\n    3\n}\n\n/// Send the request again.\npub fn send_again() {\n    retry_count();\n}\n";

#[test]
fn a_lookup_that_names_one_short_definition_prints_its_body_under_the_row() {
    let repo = scanned_repo(&[("src/net.rs", NET)]);

    let (found, text) = lookup(&repo, "send_again", 20);

    let row = text
        .find("- function `send_again` [src/net.rs:")
        .unwrap_or_else(|| panic!("{text}"));
    let body = body_text(&repo, "send_again");
    assert!(text.ends_with(&body), "{text}");
    assert!(text.find(&body).unwrap() > row, "{text}");
    assert!(body.contains("    retry_count();\n"), "{body}");
    assert_eq!(
        found.body.as_deref(),
        Some("pub fn send_again() {\n    retry_count();\n}")
    );
}

#[test]
fn a_lookup_whose_one_definition_is_over_a_quarter_of_the_budget_prints_the_row_and_a_body_hint() {
    let lines: String = (0..400)
        .map(|i| format!("    let value_{i} = {i} + retry_count();\n"))
        .collect();
    let long = format!("{NET}\npub fn rebuild_all() {{\n{lines}}}\n");
    assert!(long.len() > OUTPUT_BUDGET_CHARS / 4);
    let repo = scanned_repo(&[("src/net.rs", long.as_str())]);

    let (found, text) = lookup(&repo, "rebuild_all", 20);

    assert!(
        text.contains("- function `rebuild_all` [src/net.rs:"),
        "{text}"
    );
    assert!(!text.contains("let value_1 ="), "{text}");
    let id = &found.names[0].symbol_id;
    assert!(
        text.contains(&format!(
            "The body of `rebuild_all` has 402 lines, more than a lookup shows. Call get_symbol_body with symbol_id={id}.\n"
        )),
        "{text}"
    );
    assert_eq!(found.body, None);
}

#[test]
fn a_lookup_that_names_two_definitions_prints_rows_only() {
    let repo = scanned_repo(&[
        ("src/net.rs", NET),
        ("src/other.rs", "pub fn send_again() {\n    let x = 1;\n}\n"),
    ]);

    let (found, text) = lookup(&repo, "send_again", 20);

    assert_eq!(found.names.len(), 2, "{text}");
    assert!(!text.contains("// src/"), "{text}");
    assert!(!text.contains("get_symbol_body"), "{text}");
    assert_eq!(found.body, None);
}

#[test]
fn a_lookup_cut_by_its_limit_inside_the_exact_names_prints_rows_only() {
    let repo = scanned_repo(&[
        ("src/net.rs", NET),
        ("src/other.rs", "pub fn send_again() {\n    let x = 1;\n}\n"),
    ]);

    let (found, text) = lookup(&repo, "send_again", 1);

    assert_eq!(found.names.len(), 1, "{text}");
    assert!(!text.contains("// src/"), "{text}");
    assert_eq!(found.body, None);
}

#[test]
fn a_lookup_of_one_definition_with_imports_folds_the_imports_and_prints_the_body() {
    let repo = scanned_repo(&[
        ("pkg/__init__.py", ""),
        ("pkg/util.py", "def helper(x):\n    return x + 1\n"),
        ("pkg/app.py", "from pkg.util import helper\n\nhelper(2)\n"),
    ]);

    let (found, text) = lookup(&repo, "helper", 20);

    assert!(text.contains("1 import of `helper`"), "{text}");
    assert!(text.ends_with(&body_text(&repo, "helper")), "{text}");
    assert_eq!(
        found.body.as_deref(),
        Some("def helper(x):\n    return x + 1")
    );
}

#[test]
fn a_lookup_of_one_constant_a_member_listing_and_a_miss_print_no_body() {
    let repo = scanned_repo(&[(
        "pkg/app.py",
        "LIMIT = 3\n\nclass App:\n    def run(self):\n        return LIMIT\n",
    )]);

    for query in ["LIMIT", "App.", "zzzqqq"] {
        let (found, text) = lookup(&repo, query, 20);
        assert!(!text.contains("// pkg/"), "{query}: {text}");
        assert!(!text.contains("get_symbol_body"), "{query}: {text}");
        assert_eq!(found.body, None, "{query}");
    }
}

#[test]
fn a_qualified_lookup_of_one_method_prints_its_body() {
    let repo = scanned_repo(&[(
        "pkg/app.py",
        "class App:\n    def run(self):\n        return 1\n\nclass Other:\n    def run(self):\n        return 2\n",
    )]);

    let (found, text) = lookup(&repo, "App.run", 20);

    assert!(text.contains("        return 1\n"), "{text}");
    assert!(!text.contains("return 2"), "{text}");
    assert!(found.body.is_some());
}
