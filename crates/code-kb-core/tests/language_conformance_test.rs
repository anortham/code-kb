use code_kb_core::db::open_read_only;
use code_kb_core::edit_check::EditCheck;
use code_kb_core::ops::{
    blast_radius_op, edit_check_op, file_skeleton_op, get_context_slice_op, get_symbol_body_op,
    lookup_symbols_op,
};
use code_kb_core::queries::get_file;
use code_kb_core::reference_report::reference_report;
use code_kb_core::sync::{execute_julie_extract, scan_workspace, update_file};
use code_kb_core::{Symbol, Workspace, safe_tempdir};
use rusqlite::params;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repository_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn installed_language_ids() -> BTreeSet<String> {
    let output = execute_julie_extract(&["languages", "--json"]).unwrap();
    let report: serde_json::Value = serde_json::from_str(&output).unwrap();
    report["languages"]["languages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|language| language["language"].as_str().unwrap().to_owned())
        .collect()
}

fn fixture_language_ids() -> BTreeSet<String> {
    let root = repository_root().join("evals/languages/tests");
    fs::read_dir(root)
        .into_iter()
        .flatten()
        .map(Result::unwrap)
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect()
}

fn fixture_cases() -> serde_json::Map<String, serde_json::Value> {
    let source =
        fs::read_to_string(repository_root().join("evals/languages/fixtures.json")).unwrap();
    serde_json::from_str(&source).unwrap()
}

fn fixture_path(case: &serde_json::Value) -> &str {
    case["file"].as_str().unwrap()
}

fn fixture_symbol(case: &serde_json::Value) -> Option<&str> {
    case["symbol"].as_str()
}

fn exact_symbol(
    db_path: &Path,
    conn: &rusqlite::Connection,
    name: &str,
    path: &str,
) -> Option<Symbol> {
    lookup_symbols_op(db_path, conn, name, None, Some(path), false, 50)
        .unwrap()
        .names
        .into_iter()
        .find(|symbol| symbol.name == name && symbol.path == path)
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn commit_worktree(root: &Path, message: &str) {
    git(root, &["add", "--all"]);
    git(root, &["commit", "--quiet", "-m", message]);
}

fn copy_fixture_files(root: &Path, cases: &serde_json::Map<String, serde_json::Value>) {
    let fixture_root = repository_root().join("evals/languages");
    for case in cases.values() {
        let source = fixture_root.join(case["source"].as_str().unwrap());
        let target = root.join(fixture_path(case));
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::copy(source, target).unwrap();
    }
}

fn check_case_body(
    workspace: &Workspace,
    db_path: &Path,
    conn: &rusqlite::Connection,
    case: &serde_json::Value,
) {
    let name = case
        .get("body_symbol")
        .and_then(serde_json::Value::as_str)
        .or_else(|| fixture_symbol(case))
        .unwrap();
    let (symbol, body) =
        get_symbol_body_op(workspace, db_path, conn, name, Some(fixture_path(case))).unwrap();
    assert!(
        !body.trim().is_empty(),
        "{} body was empty",
        fixture_path(case)
    );
    if let Some(expected) = case["body_contains"].as_str() {
        assert!(
            body.contains(expected),
            "{} body did not contain {expected:?}: {body:?}",
            fixture_path(case)
        );
    }
    assert_eq!(symbol.path, fixture_path(case));
}

fn update_fixture(workspace: &Workspace, db_path: &Path, case: &serde_json::Value, language: &str) {
    let path = fixture_path(case);
    let source_path = workspace.root.join(path);
    let source = fs::read_to_string(&source_path).unwrap();
    let (old, new) = if let Some(symbol) = case
        .get("edit_symbol")
        .and_then(serde_json::Value::as_str)
        .or_else(|| fixture_symbol(case))
    {
        let replacement = case
            .get("edit_replacement")
            .and_then(serde_json::Value::as_str)
            .or_else(|| case["replacement"].as_str())
            .unwrap();
        (symbol, replacement)
    } else {
        (
            case["refresh_from"].as_str().unwrap(),
            case["refresh_to"].as_str().unwrap(),
        )
    };
    let occurrence_count = case
        .get("refresh_occurrences")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(1) as usize;
    let refresh_start = case
        .get("refresh_after")
        .and_then(serde_json::Value::as_str)
        .map(|marker| {
            source
                .find(marker)
                .map(|position| position + marker.len())
                .unwrap_or_else(|| panic!("{language} refresh marker {marker:?} was absent"))
        })
        .unwrap_or(0);
    let (prefix, tail) = source.split_at(refresh_start);
    let mut refreshed_tail = tail.to_owned();
    for _ in 0..occurrence_count {
        let next = refreshed_tail.replacen(old, new, 1);
        assert_ne!(
            refreshed_tail, next,
            "{language} fixture must contain {occurrence_count} refresh occurrences"
        );
        refreshed_tail = next;
    }
    let refreshed = format!("{prefix}{refreshed_tail}");
    assert_ne!(
        source, refreshed,
        "{language} fixture must change on refresh"
    );
    fs::write(source_path, refreshed).unwrap();
    update_file(workspace, db_path, path).unwrap();
}

fn assert_edit_detects_change(
    check: &EditCheck,
    language: &str,
    case: &serde_json::Value,
    consumer_gaps: &mut Vec<String>,
) {
    let name = case
        .get("edit_symbol")
        .and_then(serde_json::Value::as_str)
        .or_else(|| fixture_symbol(case))
        .unwrap();
    let path = fixture_path(case);
    let change = check
        .changes
        .iter()
        .find(|change| change.path == path && change.display.contains(name));
    let Some(change) = change else {
        consumer_gaps.push(format!(
            "{language} change of {name} was absent from edit check: {:#?}",
            check.changes
        ));
        return;
    };
    if let Some(caller) = case["caller"].as_str() {
        let has_unchanged_caller = change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == path && site.caller == caller);
        let has_in_diff_caller = case["caller_in_diff"].as_bool().unwrap_or(false)
            && change
                .in_diff
                .iter()
                .any(|site| site.path == path && site.caller == caller);
        if !has_unchanged_caller && !has_in_diff_caller {
            consumer_gaps.push(format!(
                "{language} edit check for changed {name} omitted caller {caller}: {change:#?}"
            ));
        }
    }
}

#[test]
fn language_fixture_directories_cover_the_installed_julie_languages() {
    let installed = installed_language_ids();
    let fixtures = fixture_language_ids();
    let cases: BTreeSet<String> = fixture_cases().into_iter().map(|(name, _)| name).collect();

    assert_eq!(
        fixtures, installed,
        "language fixture set must match pinned Julie"
    );
    assert_eq!(
        cases, installed,
        "every pinned language needs a fixture case"
    );
}

#[test]
fn real_extractor_fixtures_cover_navigation_slices_references_impact_and_refresh() {
    let cases = fixture_cases();
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    copy_fixture_files(&root, &cases);
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Language Conformance"]);
    git(
        &root,
        &[
            "config",
            "user.email",
            "language-conformance@example.invalid",
        ],
    );
    commit_worktree(&root, "baseline language fixtures");

    let workspace = Workspace::new(root.clone());
    let db_path = root.join(".code-kb/artifact.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let mut consumer_gaps = Vec::new();

    for (language, case) in &cases {
        let path = fixture_path(case);
        let file = get_file(&conn, path).unwrap().unwrap();
        assert_eq!(file.language, *language, "fixture parser for {path}");
        let skeleton = file_skeleton_op(&workspace, &db_path, &conn, path).unwrap();

        let Some(name) = fixture_symbol(case) else {
            assert!(
                skeleton.to_ascii_lowercase().contains("no symbols"),
                "{language} no-symbol fixture must report that it has no symbols: {skeleton}"
            );
            let lookup = lookup_symbols_op(
                &db_path,
                &conn,
                "ConformanceChecksum",
                None,
                Some(path),
                false,
                10,
            )
            .unwrap();
            assert!(lookup.names.is_empty());
            assert!(lookup.ranked.is_empty());
            continue;
        };

        let symbol = exact_symbol(&db_path, &conn, name, path)
            .unwrap_or_else(|| panic!("{language} did not extract {name} from {path}"));
        assert_eq!(symbol.language, *language);
        assert_eq!(symbol.kind, case["symbol_kind"].as_str().unwrap());
        if !skeleton.contains(name) {
            consumer_gaps.push(format!(
                "{language} skeleton omitted extracted {name} from {path}: {skeleton}"
            ));
        }
        check_case_body(&workspace, &db_path, &conn, case);
        let context =
            get_context_slice_op(&workspace, &db_path, &conn, name, Some(path), false).unwrap();
        assert_eq!(context.target_symbol.symbol_id, symbol.symbol_id);
        assert!(!context.target_body.trim().is_empty());

        if let Some(names) = case["skeleton_symbols"].as_array() {
            for expected_name in names.iter().filter_map(serde_json::Value::as_str) {
                if exact_symbol(&db_path, &conn, expected_name, path).is_none() {
                    consumer_gaps.push(format!(
                        "{language} lookup omitted expected skeleton symbol {expected_name} from {path}"
                    ));
                    continue;
                }
                if !skeleton.contains(expected_name) {
                    consumer_gaps.push(format!(
                        "{language} skeleton omitted extracted {expected_name} from {path}: {skeleton}"
                    ));
                }
            }
        }

        if let Some(expected) = case.get("references") {
            let source_name = expected["source_name"].as_str().unwrap();
            let target_name = expected["target_name"].as_str().unwrap();
            let target = exact_symbol(&db_path, &conn, target_name, path).unwrap();
            let relation_kind = expected["relationship_kind"].as_str().unwrap();
            let extracted_relations: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM relationships r JOIN symbols source ON source.symbol_id = r.from_symbol_id JOIN symbols target ON target.symbol_id = r.to_symbol_id WHERE r.path = ?1 AND source.name = ?2 AND target.name = ?3 AND r.kind = ?4",
                    params![path, source_name, target_name, relation_kind],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(extracted_relations, 1);
            let report = reference_report(&conn, &workspace.root, &target, false).unwrap();
            if !report
                .sites
                .iter()
                .any(|site| site.site.from_symbol_name == source_name)
            {
                consumer_gaps.push(format!(
                    "{language} reference report omitted {source_name} -> {target_name}: {report:#?}"
                ));
            }
            let impact =
                blast_radius_op(&workspace, &conn, Some(target_name), None, 2, 20).unwrap();
            if !impact
                .impacted_symbols
                .iter()
                .any(|symbol| symbol.name == source_name && symbol.path == path)
            {
                consumer_gaps.push(format!(
                    "{language} impact query omitted {source_name} -> {target_name}: {impact:#?}"
                ));
            }
        }

        if let Some(caller) = case["caller"].as_str() {
            let context =
                get_context_slice_op(&workspace, &db_path, &conn, caller, Some(path), false)
                    .unwrap();
            assert_eq!(context.target_symbol.name, caller);
            assert!(context.target_body.contains(caller));
            if !context
                .callee_signatures
                .iter()
                .any(|signature| signature.contains(name))
            {
                consumer_gaps.push(format!(
                    "{language} context for {caller} omitted callee {name}: {context:#?}"
                ));
            }

            let report = reference_report(&conn, &workspace.root, &symbol, false).unwrap();
            if !report
                .sites
                .iter()
                .any(|site| site.site.from_symbol_name == caller && site.site.path == path)
            {
                consumer_gaps.push(format!(
                    "{language} reference report omitted {caller} -> {name}: {report:#?}"
                ));
            }

            let impact = blast_radius_op(&workspace, &conn, Some(name), None, 2, 20).unwrap();
            if !impact
                .impacted_symbols
                .iter()
                .any(|symbol| symbol.name == caller && symbol.path == path)
            {
                consumer_gaps.push(format!(
                    "{language} impact query omitted {caller} -> {name}: {impact:#?}"
                ));
            }
        }

        if let Some(from) = case["reference_from"].as_str() {
            let report = reference_report(&conn, &workspace.root, &symbol, false).unwrap();
            assert!(
                report
                    .sites
                    .iter()
                    .any(|site| site.site.from_symbol_name == from && site.site.path == path),
                "{language} references omitted {from} -> {name}: {report:#?}"
            );
        }
    }

    let css = &cases["css"];
    let custom_function = &css["custom_function"];
    let path = fixture_path(css);
    let declaration_name = custom_function["declaration_name"].as_str().unwrap();
    let declaration_kind = custom_function["declaration_kind"].as_str().unwrap();
    let caller_name = custom_function["caller_name"].as_str().unwrap();
    let call_name = custom_function["call_identifier"].as_str().unwrap();
    let declaration = exact_symbol(&db_path, &conn, declaration_name, path);
    let caller = exact_symbol(&db_path, &conn, caller_name, path);
    if declaration.is_none() {
        let raw_name = "@function --anchor_css()";
        let raw = exact_symbol(&db_path, &conn, raw_name, path);
        consumer_gaps.push(format!(
            "CSS lookup omitted canonical {declaration_kind} {declaration_name}; raw Julie row was {raw:?}"
        ));
    }
    if caller.is_none() {
        let raw_name = "@function --caller_css()";
        let raw = exact_symbol(&db_path, &conn, raw_name, path);
        consumer_gaps.push(format!(
            "CSS lookup omitted canonical function {caller_name}; raw Julie row was {raw:?}"
        ));
    }
    let call_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM identifiers WHERE path = ?1 AND kind = 'call' AND name = ?2 AND containing_symbol_id = ?3",
            params![path, call_name, caller.as_ref().map(|symbol| symbol.symbol_id.as_str())],
            |row| row.get(0),
        )
        .unwrap();
    if let (Some(declaration), Some(caller)) = (declaration.as_ref(), caller.as_ref()) {
        assert_eq!(declaration.kind, declaration_kind);
        assert!(
            declaration
                .signature
                .as_deref()
                .unwrap()
                .contains("@function --anchor_css()")
        );
        assert_eq!(caller.kind, "function");
        assert_eq!(call_rows, 1);
        let report = reference_report(&conn, &workspace.root, declaration, false).unwrap();
        if !report
            .sites
            .iter()
            .any(|site| site.site.from_symbol_name == caller_name)
        {
            consumer_gaps.push(format!(
                "CSS reference report omitted {caller_name} -> {declaration_name}: {report:#?}"
            ));
        }
        let context =
            get_context_slice_op(&workspace, &db_path, &conn, &caller.name, Some(path), false)
                .unwrap();
        assert_eq!(context.target_symbol.name, caller.name);
        if !context
            .callee_signatures
            .iter()
            .any(|signature| signature.contains(declaration_name))
        {
            consumer_gaps.push(format!(
                "CSS context for {caller_name} omitted callee {declaration_name}: {context:#?}"
            ));
        }
        let impact =
            blast_radius_op(&workspace, &conn, Some(declaration_name), None, 2, 20).unwrap();
        if !impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == caller_name && symbol.path == path)
        {
            consumer_gaps.push(format!(
                "CSS impact query omitted {caller_name} -> {declaration_name}: {impact:#?}"
            ));
        }
    } else {
        assert_eq!(call_rows, 0);
    }
    let css_skeleton = file_skeleton_op(&workspace, &db_path, &conn, path).unwrap();
    for expected in css["additional_custom_functions"].as_array().unwrap() {
        let name = expected["name"].as_str().unwrap();
        let signature = expected["signature"].as_str().unwrap();
        let symbol = exact_symbol(&db_path, &conn, name, path);
        if symbol.is_none() {
            let raw = exact_symbol(&db_path, &conn, signature, path);
            consumer_gaps.push(format!(
                "CSS lookup omitted function {name}; raw Julie row was {raw:?}"
            ));
            continue;
        }
        let symbol = symbol.unwrap();
        if symbol.kind != "function"
            || !symbol
                .signature
                .as_deref()
                .is_some_and(|actual| actual.contains(signature))
        {
            consumer_gaps.push(format!(
                "CSS custom function {name} has unexpected extracted shape: {symbol:#?}"
            ));
        }
        if !css_skeleton.contains(name) {
            consumer_gaps.push(format!(
                "CSS skeleton omitted custom function {name}: {css_skeleton}"
            ));
        }
        let (_, body) = get_symbol_body_op(&workspace, &db_path, &conn, name, Some(path)).unwrap();
        assert!(
            !body.trim().is_empty(),
            "CSS custom function {name} had no body"
        );
        let context =
            get_context_slice_op(&workspace, &db_path, &conn, name, Some(path), false).unwrap();
        assert_eq!(context.target_symbol.symbol_id, symbol.symbol_id);
        assert!(!context.target_body.trim().is_empty());
    }
    for expected in css["unicode_and_escape_calls"].as_array().unwrap() {
        let target_name = expected["target"].as_str().unwrap();
        let caller_name = expected["caller"].as_str().unwrap();
        let identifier_name = expected["identifier"].as_str().unwrap();
        let target = exact_symbol(&db_path, &conn, target_name, path);
        let caller = exact_symbol(&db_path, &conn, caller_name, path);
        if target.is_none() || caller.is_none() {
            consumer_gaps.push(format!(
                "CSS Unicode/escaped call lookup omitted target {target_name} or caller {caller_name}: target={target:?}, caller={caller:?}"
            ));
            continue;
        }
        let target = target.unwrap();
        let caller = caller.unwrap();
        let mut statement = conn
            .prepare(
                "SELECT name, start_line, start_column, end_line, end_column FROM identifiers WHERE path = ?1 AND kind = 'call' AND containing_symbol_id = ?2 ORDER BY start_line, start_column",
            )
            .unwrap();
        let identifiers: Vec<(String, i64, i64, i64, i64)> = statement
            .query_map(params![path, caller.symbol_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let span = &expected["span"];
        let expected_span = (
            span["start_line"].as_i64().unwrap(),
            span["start_column"].as_i64().unwrap(),
            span["end_line"].as_i64().unwrap(),
            span["end_column"].as_i64().unwrap(),
        );
        if !identifiers
            .iter()
            .any(|(name, start_line, start_column, end_line, end_column)| {
                name == identifier_name
                    && (*start_line, *start_column, *end_line, *end_column) == expected_span
            })
        {
            consumer_gaps.push(format!(
                "CSS caller {caller_name} expected call identifier {identifier_name} at {expected_span:?}; index has {identifiers:?}"
            ));
        }
        let context =
            get_context_slice_op(&workspace, &db_path, &conn, caller_name, Some(path), false)
                .unwrap();
        if !context
            .callee_signatures
            .iter()
            .any(|signature| signature.contains(target_name))
        {
            consumer_gaps.push(format!(
                "CSS context for {caller_name} omitted callee {target_name}: {context:#?}"
            ));
        }
        let report = reference_report(&conn, &workspace.root, &target, false).unwrap();
        if !report.sites.iter().any(|site| {
            site.site.from_symbol_name == caller_name
                && site.label == code_kb_core::SiteLabel::Proven
        }) {
            consumer_gaps.push(format!(
                "CSS reference report omitted proven site {caller_name} -> {target_name}: {report:#?}"
            ));
        }
        let impact = blast_radius_op(&workspace, &conn, Some(target_name), None, 2, 20).unwrap();
        if !impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == caller_name && symbol.path == path)
        {
            consumer_gaps.push(format!(
                "CSS impact query omitted {caller_name} -> {target_name}: {impact:#?}"
            ));
        }
    }

    let no_caller_cases: Vec<_> = cases
        .iter()
        .filter(|(_, case)| {
            (fixture_symbol(case).is_some()
                && case["caller"].is_null()
                && case["edit_check"] != true)
                || fixture_symbol(case).is_none()
        })
        .collect();
    for (language, case) in no_caller_cases {
        update_fixture(&workspace, &db_path, case, language);
        if let Some(name) = fixture_symbol(case) {
            assert!(exact_symbol(&db_path, &conn, name, fixture_path(case)).is_none());
            let replacement = case["replacement"].as_str().unwrap();
            assert!(exact_symbol(&db_path, &conn, replacement, fixture_path(case)).is_some());
        } else {
            let refreshed = get_file(&conn, fixture_path(case)).unwrap().unwrap();
            assert_eq!(refreshed.language, *language);
            let symbol_count: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM symbols WHERE path = ?1",
                    [fixture_path(case)],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(symbol_count, 0, "{language} refresh created symbols");
            let source = fs::read_to_string(root.join(fixture_path(case))).unwrap();
            assert!(source.contains(case["refresh_to"].as_str().unwrap()));
            assert!(!source.contains(case["refresh_from"].as_str().unwrap()));
        }
    }
    commit_worktree(&root, "refresh declarative fixtures");

    let edit_cases: Vec<_> = cases
        .iter()
        .filter(|(_, case)| {
            case["caller"].is_string()
                || case["reference_from"].is_string()
                || case["edit_check"] == true
        })
        .collect();
    for (group_index, group) in edit_cases.chunks(20).enumerate() {
        for (language, case) in group {
            update_fixture(&workspace, &db_path, case, language);
            let name = case
                .get("edit_symbol")
                .and_then(serde_json::Value::as_str)
                .or_else(|| fixture_symbol(case))
                .unwrap();
            assert!(
                exact_symbol(&db_path, &conn, name, fixture_path(case)).is_none(),
                "{language} refresh retained old symbol {name}"
            );
            let replacement = case
                .get("edit_replacement")
                .and_then(serde_json::Value::as_str)
                .or_else(|| case["replacement"].as_str())
                .unwrap();
            assert!(exact_symbol(&db_path, &conn, replacement, fixture_path(case)).is_some());
            if *language == "css" {
                assert!(exact_symbol(&db_path, &conn, ".button", fixture_path(case)).is_some());
                let custom_function = &case["custom_function"];
                let declaration_name = custom_function["declaration_name"].as_str().unwrap();
                let caller_name = custom_function["caller_name"].as_str().unwrap();
                assert!(
                    exact_symbol(&db_path, &conn, declaration_name, fixture_path(case)).is_some()
                );
                assert!(exact_symbol(&db_path, &conn, caller_name, fixture_path(case)).is_some());
                for expected in case["additional_custom_functions"].as_array().unwrap() {
                    let name = expected["name"].as_str().unwrap();
                    if exact_symbol(&db_path, &conn, name, fixture_path(case)).is_none() {
                        consumer_gaps.push(format!("CSS refresh omitted custom function {name}"));
                    }
                }
            }
        }
        let check = edit_check_op(&workspace, &db_path, &conn, None).unwrap();
        for (language, case) in group {
            assert_edit_detects_change(&check, language, case, &mut consumer_gaps);
            if *language == "css" {
                let old_name = case["edit_symbol"].as_str().unwrap();
                let expected_source = case["references"]["source_name"].as_str().unwrap();
                let change = check.changes.iter().find(|change| {
                    change.path == fixture_path(case) && change.display.contains(old_name)
                });
                if !change.is_some_and(|change| {
                    change.proven.iter().any(|site| {
                        site.caller == expected_source && site.path == fixture_path(case)
                    })
                }) {
                    consumer_gaps.push(format!(
                        "CSS edit check for {old_name} omitted proven reference from {expected_source}: {change:#?}"
                    ));
                }
            }
        }
        commit_worktree(
            &root,
            &format!("refresh callable fixture group {group_index}"),
        );
    }

    let css = &cases["css"];
    let css_path = fixture_path(css);
    let css_source_path = root.join(css_path);
    let css_source = fs::read_to_string(&css_source_path).unwrap();
    let repaired_reference = css_source.replacen(
        css["edit_symbol"].as_str().unwrap(),
        css["edit_replacement"].as_str().unwrap(),
        1,
    );
    assert_ne!(css_source, repaired_reference);
    fs::write(&css_source_path, repaired_reference).unwrap();
    update_file(&workspace, &db_path, css_path).unwrap();
    commit_worktree(&root, "refresh css custom property reference");

    let css_source = fs::read_to_string(&css_source_path).unwrap();
    let selector_name = css["symbol"].as_str().unwrap();
    let selector_replacement = css["selector_edit_replacement"].as_str().unwrap();
    let renamed_selector = css_source.replacen(selector_name, selector_replacement, 1);
    assert_ne!(css_source, renamed_selector);
    fs::write(&css_source_path, renamed_selector).unwrap();
    update_file(&workspace, &db_path, css_path).unwrap();
    assert!(exact_symbol(&db_path, &conn, selector_name, css_path).is_none());
    assert!(exact_symbol(&db_path, &conn, selector_replacement, css_path).is_some());
    let check = edit_check_op(&workspace, &db_path, &conn, None).unwrap();
    let selector_change = check.changes.iter().find(|change| {
        change.path == css_path
            && change.display.contains(selector_name)
            && change.change.to_ascii_lowercase().contains("renamed")
    });
    if selector_change.is_none() {
        consumer_gaps.push(format!(
            "CSS edit check omitted selector rename {selector_name} -> {selector_replacement}: {:#?}",
            check.changes
        ));
    }

    assert!(
        consumer_gaps.is_empty(),
        "consumer conformance gaps:\n{}",
        consumer_gaps.join("\n")
    );
}

#[test]
fn css_custom_function_call_evidence_agrees_and_missing_identifier_stays_unresolved() {
    let case = fixture_cases().remove("css").unwrap();
    let mut cases = serde_json::Map::new();
    cases.insert("css".to_owned(), case.clone());
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    copy_fixture_files(&root, &cases);
    let missing_fact_path = "css/no-call-identifier.css";
    fs::copy(
        repository_root().join("evals/languages/tests/css/no-call-identifier.css"),
        root.join(missing_fact_path),
    )
    .unwrap();
    git(&root, &["init", "--quiet", "--initial-branch=main"]);
    git(&root, &["config", "user.name", "Language Conformance"]);
    git(
        &root,
        &[
            "config",
            "user.email",
            "language-conformance@example.invalid",
        ],
    );
    commit_worktree(&root, "baseline css fixture");

    let workspace = Workspace::new(root.clone());
    let db_path = root.join(".code-kb/artifact.db");
    scan_workspace(&workspace, &db_path, true).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let path = fixture_path(&case);
    let custom_function = &case["custom_function"];
    let declaration_name = custom_function["declaration_name"].as_str().unwrap();
    let caller_name = custom_function["caller_name"].as_str().unwrap();
    let declaration = exact_symbol(&db_path, &conn, declaration_name, path).unwrap();
    let caller = exact_symbol(&db_path, &conn, caller_name, path).unwrap();
    let identifier_rows: Vec<(String, String, Option<String>, i64)> = {
        let mut statement = conn
            .prepare("SELECT name, kind, containing_symbol_id, start_line FROM identifiers WHERE path = ?1 AND containing_symbol_id = ?2 ORDER BY start_line, start_column")
            .unwrap();
        statement
            .query_map(params![path, caller.symbol_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    let report = reference_report(&conn, &workspace.root, &declaration, false).unwrap();
    let context =
        get_context_slice_op(&workspace, &db_path, &conn, caller_name, Some(path), false).unwrap();
    let impact = blast_radius_op(&workspace, &conn, Some(declaration_name), None, 2, 20).unwrap();
    let has_reference = report
        .sites
        .iter()
        .any(|site| site.site.from_symbol_name == caller_name && site.site.path == path);
    let has_callee = context
        .callee_signatures
        .iter()
        .any(|signature| signature.contains(declaration_name));
    let has_impact = impact
        .impacted_symbols
        .iter()
        .any(|symbol| symbol.name == caller_name && symbol.path == path);
    if std::env::var_os("CODE_KB_CONFORMANCE_EVIDENCE").is_some() {
        eprintln!(
            "CSS evidence root={} selected={declaration_name} file={path}\nidentifier rows: {identifier_rows:#?}\nselected target: {declaration:#?}\nReferenceReport: {report:#?}\nContextSlice: {context:#?}\nBlastRadius: {impact:#?}",
            root.display()
        );
    }

    assert!(
        has_reference && has_callee && has_impact,
        "CSS call evidence disagreed for {} in {}\nidentifier rows: {identifier_rows:#?}\nselected target: {declaration:#?}\nReferenceReport: {report:#?}\nContextSlice: {context:#?}\nBlastRadius: {impact:#?}",
        declaration_name,
        path
    );

    let missing_target = exact_symbol(&db_path, &conn, "--foo名", missing_fact_path).unwrap();
    let shorter_target = exact_symbol(&db_path, &conn, "--foo", missing_fact_path).unwrap();
    let missing_caller = exact_symbol(&db_path, &conn, "--caller_css", missing_fact_path).unwrap();
    let missing_identifiers: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM identifiers WHERE path = ?1 AND kind = 'call' AND containing_symbol_id = ?2",
            params![missing_fact_path, missing_caller.symbol_id],
            |row| row.get(0),
        )
        .unwrap();
    let missing_report = reference_report(&conn, &workspace.root, &missing_target, false).unwrap();
    let shorter_report = reference_report(&conn, &workspace.root, &shorter_target, false).unwrap();
    let missing_context = get_context_slice_op(
        &workspace,
        &db_path,
        &conn,
        "--caller_css",
        Some(missing_fact_path),
        false,
    )
    .unwrap();
    let missing_impact = blast_radius_op(&workspace, &conn, Some("--foo名"), None, 2, 20).unwrap();
    let shorter_impact = blast_radius_op(&workspace, &conn, Some("--foo"), None, 2, 20).unwrap();
    let has_missing_fact_reference = missing_report
        .sites
        .iter()
        .any(|site| site.site.from_symbol_name == "--caller_css");
    let has_missing_fact_callee = missing_context
        .callee_signatures
        .iter()
        .any(|signature| signature.contains("--foo名"));
    let has_missing_fact_impact = missing_impact
        .impacted_symbols
        .iter()
        .any(|symbol| symbol.name == "--caller_css" && symbol.path == missing_fact_path);
    let has_shorter_fact_reference = shorter_report
        .sites
        .iter()
        .any(|site| site.site.from_symbol_name == "--caller_css");
    let has_shorter_fact_impact = shorter_impact
        .impacted_symbols
        .iter()
        .any(|symbol| symbol.name == "--caller_css" && symbol.path == missing_fact_path);
    if std::env::var_os("CODE_KB_CONFORMANCE_EVIDENCE").is_some() {
        eprintln!(
            "CSS missing-fact evidence path={missing_fact_path}\ncall identifiers: {missing_identifiers}\nReferenceReport for --foo名: {missing_report:#?}\nReferenceReport for --foo: {shorter_report:#?}\nContextSlice: {missing_context:#?}\nBlastRadius for --foo名: {missing_impact:#?}\nBlastRadius for --foo: {shorter_impact:#?}"
        );
    }
    assert_eq!(missing_identifiers, 0);
    assert!(!has_missing_fact_reference);
    assert!(!has_missing_fact_callee);
    assert!(!has_missing_fact_impact);
    assert!(!has_shorter_fact_reference);
    assert!(!has_shorter_fact_impact);
}
