use std::fs;
use std::path::PathBuf;

use code_kb_core::Symbol;
use code_kb_core::db::open_read_only;
use code_kb_core::models::{LabeledSite, SiteLabel};
use code_kb_core::ops::{
    blast_radius_op, get_context_slice_op, get_symbol_body_op, lookup_symbols_op,
};
use code_kb_core::reference_report::{reference_report_for_impact, select_reference_target};
use code_kb_core::safe_tempdir;
use code_kb_core::sync::{scan_workspace, update_file};
use code_kb_core::workspace::Workspace;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

impl Repo {
    fn workspace(&self) -> Workspace {
        Workspace::new(self.root.clone())
    }

    fn function(&self, name: &str) -> Symbol {
        let conn = open_read_only(&self.db).unwrap();
        lookup_symbols_op(&self.db, &conn, name, None, None, false, 20)
            .unwrap()
            .names
            .into_iter()
            .find(|symbol| symbol.name == name)
            .unwrap_or_else(|| panic!("lookup found no row named {name:?}"))
    }

    fn sites(&self, name: &str, path: Option<&str>) -> Vec<LabeledSite> {
        let conn = open_read_only(&self.db).unwrap();
        let target = select_reference_target(&conn, name, path).unwrap();
        reference_report_for_impact(&conn, &self.root, &target)
            .unwrap()
            .sites
    }

    fn callees(&self, caller: &str, path: &str) -> Vec<String> {
        let conn = open_read_only(&self.db).unwrap();
        get_context_slice_op(
            &self.workspace(),
            &self.db,
            &conn,
            caller,
            Some(path),
            false,
        )
        .unwrap()
        .callee_signatures
    }

    fn impacted(&self, name: &str) -> Vec<(String, String)> {
        let conn = open_read_only(&self.db).unwrap();
        blast_radius_op(&self.workspace(), &conn, Some(name), None, 2, 20)
            .unwrap()
            .impacted_symbols
            .into_iter()
            .map(|symbol| (symbol.name, symbol.path))
            .collect()
    }

    fn call_names(&self, path: &str) -> Vec<(String, i64, Option<i64>)> {
        let conn = open_read_only(&self.db).unwrap();
        conn.prepare(
            "SELECT name, start_line,
                    CASE WHEN json_valid(metadata_json)
                         THEN json_extract(metadata_json, '$.arg_count') END
             FROM identifiers WHERE path = ?1 AND language = 'css' AND kind = 'call'
             ORDER BY start_line, start_column",
        )
        .unwrap()
        .query_map([path], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
    }
}

fn scanned(files: &[(&str, &str)]) -> Repo {
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, source) in files {
        fs::write(root.join(path), source).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).unwrap();
    Repo {
        root,
        db,
        _temp: temp,
    }
}

const FUNCTIONS: &str = "@function --anchor_css() { result: 41; }\n@function --caller_css() { result: --anchor_css(); }\n@function --parameterized_css(--value) { result: var(--value); }\n@function --名_css() { result: 29; }\n@function --escaped\\ name_css() { result: 1; }\n@function not_css() { result: 2; }\n@function --caller_unicode() { result: --名_css(); }\n@function --caller_escaped() { result: --escaped\\ name_css(); }\n@function --\u{2000}_css() { result: 30; }\n.card { --theme_css: red; color: var(--theme_css); }\n@media (min-width: 1px) { .wide { color: blue; } }\n";

#[test]
fn a_css_custom_function_is_a_function_named_by_its_decoded_name() {
    let repo = scanned(&[("functions.css", FUNCTIONS)]);
    let anchor = repo.function("--anchor_css");
    assert_eq!(anchor.kind, "function");
    let conn = open_read_only(&repo.db).unwrap();
    let (body_symbol, body) = get_symbol_body_op(
        &repo.workspace(),
        &repo.db,
        &conn,
        "--anchor_css",
        Some("functions.css"),
    )
    .unwrap();
    assert_eq!(body_symbol.symbol_id, anchor.symbol_id);
    assert_eq!(body, "@function --anchor_css() { result: 41; }");

    for (name, signature) in [
        (
            "--parameterized_css",
            "@function --parameterized_css(--value)",
        ),
        ("--名_css", "@function --名_css()"),
        ("--escaped name_css", "@function --escaped\\ name_css()"),
        ("--\u{2000}_css", "@function --\u{2000}_css()"),
    ] {
        let function = repo.function(name);
        assert_eq!(function.kind, "function", "{name}");
        assert!(
            function
                .signature
                .as_deref()
                .unwrap()
                .starts_with(signature),
            "{name}: {:?}",
            function.signature
        );
    }
    assert_eq!(repo.function("@function not_css()").kind, "namespace");
    assert_eq!(repo.function("--theme_css").kind, "property");
    assert_eq!(repo.function(".card").kind, "property");
    assert_eq!(repo.function("@media (min-width: 1px)").kind, "namespace");
}

#[test]
fn a_css_call_names_the_function_it_calls_with_its_decoded_name() {
    let repo = scanned(&[("functions.css", FUNCTIONS)]);
    let calls: Vec<_> = repo
        .call_names("functions.css")
        .into_iter()
        .filter(|(_, _, args)| args.is_some())
        .collect();
    assert_eq!(
        calls,
        vec![
            ("--anchor_css".to_string(), 2, Some(0)),
            ("--名_css".to_string(), 7, Some(0)),
            ("--escaped name_css".to_string(), 8, Some(0)),
        ]
    );
}

#[test]
fn a_css_call_reaches_its_function_in_the_same_stylesheet() {
    let repo = scanned(&[("functions.css", FUNCTIONS)]);
    for (target, written, caller, line) in [
        ("--anchor_css", "--anchor_css", "--caller_css", 2),
        ("--名_css", "--名_css", "--caller_unicode", 7),
        (
            "--escaped name_css",
            "--escaped\\ name_css",
            "--caller_escaped",
            8,
        ),
    ] {
        let sites = repo.sites(target, None);
        assert!(
            sites.iter().any(|site| site.label == SiteLabel::Proven
                && site.site.from_symbol_name == caller
                && site.site.start_line == Some(line)),
            "{target}: {sites:#?}"
        );
        assert!(
            repo.callees(caller, "functions.css")
                .iter()
                .any(|signature| signature.contains(written)),
            "{caller}"
        );
        assert!(
            repo.impacted(target)
                .contains(&(caller.to_string(), "functions.css".to_string())),
            "{target}"
        );
    }
}

#[test]
fn a_result_call_with_a_longer_unicode_name_reaches_only_that_function() {
    let repo = scanned(&[(
        "functions.css",
        "@function --foo() { result: 1; }\n@function --foo名() { result: 2; }\n@function --caller_css() { result: --foo名(); }\n",
    )]);
    let longer = repo.sites("--foo名", None);
    assert!(
        longer.iter().any(|site| site.label == SiteLabel::Proven
            && site.site.from_symbol_name == "--caller_css"
            && site.site.start_line == Some(3)),
        "{longer:#?}"
    );
    assert!(repo.sites("--foo", None).is_empty());
    assert!(
        repo.impacted("--foo名")
            .contains(&("--caller_css".to_string(), "functions.css".to_string()))
    );
    assert!(repo.impacted("--foo").is_empty());
}

const THEME: &str = "@function --shared(--a, --b) { result: calc(var(--a) + var(--b)); }\n@function --\u{2000}_css() { result: 1; }\n";
const PAGE: &str = ".card { width: --shared(1, 2); }\n.wide { height: --\u{2000}_css(); }\n@function --page_css() { result: --shared(3, 4); }\n";

#[test]
fn a_css_call_reaches_a_function_declared_in_another_stylesheet() {
    let repo = scanned(&[("theme.css", THEME), ("page.css", PAGE)]);
    for (target, caller, line, args) in [
        ("--shared", ".card", 1, 2),
        ("--\u{2000}_css", ".wide", 2, 0),
        ("--shared", "--page_css", 3, 2),
    ] {
        let sites = repo.sites(target, None);
        let site = sites
            .iter()
            .find(|site| site.site.from_symbol_name == caller && site.site.path == "page.css")
            .unwrap_or_else(|| panic!("{target} has no site in {caller}: {sites:#?}"));
        assert_eq!(site.label, SiteLabel::Likely, "{site:#?}");
        assert_eq!(site.site.start_line, Some(line));
        assert_eq!(site.arg_count, Some(args));
        assert!(
            repo.callees(caller, "page.css")
                .iter()
                .any(|signature| signature.contains(target)),
            "{caller}"
        );
        assert!(
            repo.impacted(target)
                .contains(&(caller.to_string(), "page.css".to_string())),
            "{target}"
        );
    }
}

#[test]
fn a_css_call_that_two_other_stylesheets_can_satisfy_is_unresolved() {
    let repo = scanned(&[
        ("theme.css", THEME),
        (
            "dark.css",
            "@function --shared(--a) { result: var(--a); }\n",
        ),
        ("page.css", PAGE),
    ]);
    let sites = repo.sites("--shared", Some("theme.css"));
    let card = sites
        .iter()
        .find(|site| site.site.from_symbol_name == ".card")
        .unwrap_or_else(|| panic!("{sites:#?}"));
    assert_eq!(card.label, SiteLabel::Unresolved, "{card:#?}");
}

#[test]
fn an_updated_stylesheet_moves_a_call_to_the_renamed_function() {
    let repo = scanned(&[("theme.css", THEME), ("page.css", PAGE)]);
    fs::write(
        repo.root.join("theme.css"),
        THEME.replace("--shared(--a", "--joined(--a"),
    )
    .unwrap();
    update_file(&repo.workspace(), &repo.db, "theme.css").unwrap();
    assert_eq!(repo.function("--joined").kind, "function");
    assert!(repo.sites("--joined", None).is_empty());

    fs::write(
        repo.root.join("page.css"),
        PAGE.replace("--shared", "--joined"),
    )
    .unwrap();
    update_file(&repo.workspace(), &repo.db, "page.css").unwrap();
    let sites = repo.sites("--joined", None);
    assert!(
        sites
            .iter()
            .any(|site| site.site.from_symbol_name == ".card" && site.site.path == "page.css"),
        "{sites:#?}"
    );
}
