use code_kb_core::{
    Workspace, compute_blast_radius, file_skeleton_op, find_julie_extract_binary,
    find_references_for_symbol, find_references_scoped, format_blast_radius, format_references,
    fts_search_symbols_scoped, open_read_only, open_read_write, safe_tempdir, scan_workspace,
    search_symbols_scoped,
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

#[test]
fn member_access_rejects_a_different_receiver() {
    let (_repo, db) = scanned_repo(&[
        (
            "web/runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n\nclass Shell:\n    def invoke(self):\n        pass\n",
        ),
        (
            "web/builders.py",
            "from web.runners import Runner, Shell\n\ndef make_runner() -> Runner:\n    return Runner()\n\ndef make_shell() -> Shell:\n    return Shell()\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.runners import Runner\n\n@pytest.fixture\ndef runner():\n    return Runner()\n",
        ),
        (
            "other/tests/conftest.py",
            "import pytest\nfrom web.runners import Shell\n\n@pytest.fixture\ndef runner():\n    return Shell()\n",
        ),
        (
            "tests/test_cli.py",
            "from web.builders import make_runner, make_shell\n\ndef test_local():\n    runner = make_runner()\n    return runner.invoke\n\ndef test_fixture(runner):\n    return runner.invoke\n\ndef test_wrong():\n    runner = make_shell()\n    return runner.invoke\n",
        ),
        (
            "other/tests/test_cli.py",
            "def test_other_fixture(runner):\n    return runner.invoke\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let callers = |name| {
        let rows = find_references_scoped(&conn, name, "callers", 20, false, None).unwrap();
        assert!(
            rows.iter().all(|row| row.kind == "member_access"),
            "{rows:?}"
        );
        caller_names(&rows)
    };

    assert_eq!(callers("Runner.invoke"), ["test_fixture", "test_local"]);
    assert_eq!(
        callers("Shell.invoke"),
        ["test_other_fixture", "test_wrong"]
    );
}

#[test]
fn resolved_same_file_builder_calls_disambiguate_annotated_receivers() {
    let (_repo, db) = scanned_repo(&[
        (
            "pkg/server.py",
            "class Server:\n    def ctx(self):\n        pass\n",
        ),
        (
            "other/server.py",
            "class Server:\n    def ctx(self):\n        pass\n",
        ),
        (
            "pkg/cli.py",
            "from pkg.server import Server\n\ndef load() -> Server:\n    return Server()\n\ndef run():\n    app = load()\n    app.ctx()\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();

    let package_server = find_references_scoped(
        &conn,
        "Server.ctx",
        "callers",
        20,
        false,
        Some("pkg/server.py"),
    )
    .unwrap();
    let rival_server = find_references_scoped(
        &conn,
        "Server.ctx",
        "callers",
        20,
        false,
        Some("other/server.py"),
    )
    .unwrap();

    assert_eq!(caller_names(&package_server), ["run"]);
    assert!(rival_server.is_empty(), "{rival_server:?}");
}

#[test]
fn a_nested_same_file_helper_call_keeps_the_constructor_receiver_supported() {
    let (_repo, db) = scanned_repo(&[
        (
            "pkg/cfg.py",
            "class Config:\n    def run(self):\n        pass\n",
        ),
        (
            "pkg/app.py",
            "from pkg.cfg import Config\n\ndef make_name():\n    return 'x'\n\ndef other(register):\n    c = Config(make_name())\n    register(c.run)\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();

    let rows =
        find_references_scoped(&conn, "run", "callers", 20, false, Some("pkg/cfg.py")).unwrap();

    assert_eq!(caller_names(&rows), ["other"]);
    assert!(
        rows.iter().all(|row| row.kind == "member_access"),
        "{rows:?}"
    );
}

#[test]
fn name_only_type_usages_cross_the_javascript_family_languages() {
    let (_repo, db) = scanned_repo(&[
        ("src/model.ts", "export class Store {\n  size = 0;\n}\n"),
        (
            "src/view.tsx",
            "import { Store } from './model';\nexport function View(p: { store: Store }) {\n  const s: Store = p.store;\n  return s;\n}\n",
        ),
        (
            "src/other.ts",
            "import { Store } from './model';\nexport function other(): Store {\n  return new Store();\n}\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let store_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE path = 'src/model.ts' AND name = 'Store' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let rows = find_references_for_symbol(&conn, "Store", "callers", 20, &store_id).unwrap();

    assert!(
        rows.iter()
            .any(|row| row.kind == "type_usage" && row.path == "src/view.tsx"),
        "{rows:?}"
    );
}

#[test]
fn resolved_same_file_builder_calls_keep_lexical_identity() {
    let (_repo, db) = scanned_repo(&[(
        "app.py",
        "class A:\n    def ctx(self):\n        pass\n\nclass B:\n    def ctx(self):\n        pass\n\ndef load() -> A:\n    return A()\n\ndef run():\n    app = load()\n    app.ctx()\n    return app.ctx\n\ndef outer():\n    def load() -> B:\n        return B()\n    app = load()\n    app.ctx()\n    return app.ctx\n",
    )]);
    let conn = open_read_only(&db).unwrap();
    let callers = |target| {
        find_references_scoped(&conn, target, "callers", 20, false, Some("app.py")).unwrap()
    };

    let a_callers = callers("A.ctx");
    let b_callers = callers("B.ctx");
    assert_eq!(caller_names(&a_callers), ["run"]);
    assert_eq!(caller_names(&b_callers), ["outer"]);
    assert_eq!(
        caller_names(
            &a_callers
                .iter()
                .filter(|row| row.kind == "member_access")
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["run"]
    );
    assert_eq!(
        caller_names(
            &b_callers
                .iter()
                .filter(|row| row.kind == "member_access")
                .cloned()
                .collect::<Vec<_>>()
        ),
        ["outer"]
    );
}

#[test]
fn inferred_same_file_builder_returns_disambiguate_flask_receivers() {
    let (_repo, db) = scanned_repo(&[
        (
            "pkg/flask.py",
            "class Flask:\n    def test_client(self):\n        pass\n",
        ),
        (
            "other/flask.py",
            "class Flask:\n    def test_client(self):\n        pass\n",
        ),
        (
            "pkg/app.py",
            "from pkg.flask import Flask\n\ndef create_app():\n    app = Flask()\n    return app\n\ndef run():\n    app = create_app()\n    app.test_client()\n",
        ),
    ]);
    // The committed extractor pin predates inferredReturnType; model that newer
    // metadata here so this consumer regression also runs against the pinned binary.
    let conn = open_read_write(&db).unwrap();
    let updated = conn
        .execute(
            "UPDATE symbols
             SET metadata_json = json_set(COALESCE(metadata_json, '{}'),
                                         '$.returnType', '',
                                         '$.inferredReturnType', 'Flask')
             WHERE name = 'create_app' AND path = 'pkg/app.py' AND kind = 'function'",
            [],
        )
        .unwrap();
    assert_eq!(updated, 1);
    drop(conn);
    let conn = open_read_only(&db).unwrap();

    let package_flask = find_references_scoped(
        &conn,
        "Flask.test_client",
        "callers",
        20,
        false,
        Some("pkg/flask.py"),
    )
    .unwrap();
    let rival_flask = find_references_scoped(
        &conn,
        "Flask.test_client",
        "callers",
        20,
        false,
        Some("other/flask.py"),
    )
    .unwrap();

    assert_eq!(caller_names(&package_flask), ["run"]);
    assert!(rival_flask.is_empty(), "{rival_flask:?}");
}

#[test]
fn a_relative_package_import_keeps_the_class_that_a_builder_returns() {
    let (_repo, db) = scanned_repo(&[
        (
            "pkg/app.py",
            "class Server:\n    def ctx(self):\n        pass\n",
        ),
        ("pkg/__init__.py", "from .app import Server\n"),
        (
            "pkg/factory.py",
            "from .app import Server\n\ndef load() -> Server:\n    from . import Server\n    return Server()\n",
        ),
        (
            "pkg/cli.py",
            "from .factory import load\n\ndef run():\n    app = load()\n    app.ctx()\n",
        ),
        (
            "other/app.py",
            "class Server:\n    def ctx(self):\n        pass\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let callers = |path| {
        let rows =
            find_references_scoped(&conn, "Server.ctx", "callers", 20, false, Some(path)).unwrap();
        caller_names(&rows)
    };

    assert_eq!(callers("pkg/app.py"), ["run"]);
    assert!(callers("other/app.py").is_empty());
}

#[test]
fn a_class_defined_after_a_same_named_import_is_the_one_a_builder_returns() {
    let (_repo, db) = scanned_repo(&[
        (
            "other/runner.py",
            "class Runner:\n    def invoke(self):\n        pass\n",
        ),
        (
            "client.py",
            "from other.runner import Runner\n\nclass Runner:\n    def invoke(self):\n        pass\n\ndef make() -> Runner:\n    return Runner()\n",
        ),
        (
            "use.py",
            "from client import make\n\ndef use():\n    runner = make()\n    runner.invoke()\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let callers = |path| {
        let rows = find_references_scoped(&conn, "Runner.invoke", "callers", 20, false, Some(path))
            .unwrap();
        caller_names(&rows)
    };

    assert_eq!(callers("client.py"), ["use"]);
    assert!(callers("other/runner.py").is_empty());
}

#[test]
fn a_base_instance_does_not_reference_a_subclass_override() {
    let (_repo, db) = scanned_repo(&[
        (
            "web/testing.py",
            "from click.testing import CliRunner\n\nclass FlaskCliRunner(CliRunner):\n    def invoke(self):\n        pass\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom click.testing import CliRunner\nfrom web.testing import FlaskCliRunner\n\n@pytest.fixture\ndef runner():\n    return CliRunner()\n\n@pytest.fixture\ndef flask_runner():\n    return FlaskCliRunner()\n",
        ),
        (
            "tests/test_cli.py",
            "from click.testing import CliRunner\nfrom web.testing import FlaskCliRunner\n\ndef test_base_fixture(runner):\n    return runner.invoke\n\ndef test_subclass_fixture(flask_runner):\n    return flask_runner.invoke\n\ndef test_base_local():\n    runner = CliRunner()\n    return runner.invoke\n\ndef test_subclass_local():\n    runner = FlaskCliRunner()\n    return runner.invoke\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let rows =
        find_references_scoped(&conn, "FlaskCliRunner.invoke", "callers", 20, false, None).unwrap();

    assert_eq!(
        caller_names(&rows),
        ["test_subclass_fixture", "test_subclass_local"]
    );
}

#[test]
fn member_access_uses_the_nearest_inherited_definition() {
    let (_repo, db) = scanned_repo(&[
        (
            "web/runners.py",
            "class Base:\n    def invoke(self):\n        pass\n\nclass Middle(Base):\n    def invoke(self):\n        pass\n\n    def read_own(self):\n        return self.invoke\n\nclass Leaf(Middle):\n    def read_inherited(self):\n        return self.invoke\n\n    def read_super(self):\n        return super().invoke()\n",
        ),
        (
            "clients.py",
            "from web.runners import Base, Leaf\n\ndef read_base():\n    runner = Base()\n    return runner.invoke\n\ndef read_leaf():\n    runner = Leaf()\n    return runner.invoke\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let base = find_references_scoped(&conn, "Base.invoke", "callers", 20, false, None).unwrap();
    let middle =
        find_references_scoped(&conn, "Middle.invoke", "callers", 20, false, None).unwrap();

    assert_eq!(caller_names(&base), ["read_base"]);
    assert_eq!(
        caller_names(&middle),
        ["read_inherited", "read_leaf", "read_own", "read_super"]
    );
}

#[test]
fn unresolved_member_access_is_a_candidate() {
    let (_repo, db) = scanned_repo(&[
        (
            "web/runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n\nclass Shell:\n    def invoke(self):\n        pass\n",
        ),
        (
            "clients.py",
            "from web.runners import Runner, Shell\nfrom external import make_runner\n\ndef unknown(value):\n    return value.invoke\n\ndef untyped():\n    value = make_runner()\n    return value.invoke\n\ndef rebound(flag):\n    if flag:\n        value = Runner()\n    else:\n        value = Shell()\n    return value.invoke\n\ndef mixed(flag):\n    if flag:\n        value = Runner()\n    else:\n        value = make_runner()\n    return value.invoke\n\ndef conditional(flag):\n    value = Runner() if flag else make_runner()\n    return value.invoke\n\ndef later(value):\n    saved = value.invoke\n    value = Shell()\n    return saved\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.runners import Runner\nfrom external import make_runner\n\n@pytest.fixture\ndef mixed_runner(flag):\n    return Runner() if flag else make_runner()\n",
        ),
        (
            "tests/test_flow.py",
            "def test_mixed_fixture(mixed_runner):\n    return mixed_runner.invoke\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    for target in ["Runner.invoke", "Shell.invoke"] {
        let rows = find_references_scoped(&conn, target, "callers", 20, false, None).unwrap();
        assert_eq!(
            caller_names(&rows),
            [
                "conditional",
                "later",
                "mixed",
                "rebound",
                "test_mixed_fixture",
                "unknown",
                "untyped"
            ]
        );
        assert!(
            rows.iter()
                .all(|row| row.kind == "member_access (candidate)"),
            "{rows:?}"
        );
    }
}

#[test]
fn supported_member_access_precedes_candidates_at_the_limit() {
    let (_repo, db) = scanned_repo(&[
        (
            "web/runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n\nclass Shell:\n    def invoke(self):\n        pass\n",
        ),
        (
            "a_unknown.py",
            "def unknown(value):\n    return value.invoke\n",
        ),
        (
            "b_wrong.py",
            "from web.runners import Shell\n\ndef mismatch():\n    value = Shell()\n    return value.invoke\n",
        ),
        (
            "z_supported.py",
            "from web.runners import Runner\n\ndef known():\n    value = Runner()\n    return value.invoke, value.invoke\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let id = member_of(&conn, "Runner", "invoke");
    let rows = find_references_scoped(
        &conn,
        "Runner.invoke",
        "callers",
        1,
        false,
        Some("web/runners.py"),
    )
    .unwrap();
    assert_eq!(caller_names(&rows), ["known"]);
    assert_eq!(rows[0].kind, "member_access");
    let rows =
        code_kb_core::find_references_for_symbol(&conn, "invoke", "callers", 2, &id).unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row.from_symbol_name.as_str())
            .collect::<Vec<_>>(),
        ["known", "unknown"]
    );
    assert_eq!(rows[1].kind, "member_access (candidate)");
    assert!(
        find_references_scoped(&conn, "Runner.invoke", "callers", 0, false, None)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn find_references_stops_receiver_checks_after_filling_the_limit() {
    let unresolved = (0..1_000)
        .map(|index| format!("def unresolved_{index}(value):\n    return value.invoke\n"))
        .collect::<String>();
    let (_repo, db) = scanned_repo(&[
        (
            "web/runners.py",
            "class Runner:\n    def invoke(self):\n        pass\n",
        ),
        (
            "a_supported.py",
            "from web.runners import Runner\n\ndef known():\n    value = Runner()\n    return value.invoke\n",
        ),
        ("z_unresolved.py", &unresolved),
    ]);
    let conn = open_read_only(&db).unwrap();
    let id = member_of(&conn, "Runner", "invoke");
    let operations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = operations.clone();
    conn.progress_handler(
        100,
        Some(move || observed.fetch_add(100, std::sync::atomic::Ordering::Relaxed) >= 200_000),
    )
    .unwrap();
    let result = find_references_for_symbol(&conn, "invoke", "callers", 1, &id);
    conn.progress_handler(0, None::<fn() -> bool>).unwrap();
    let rows = result.expect("one supported caller should fit within the receiver-check budget");

    assert_eq!(caller_names(&rows), ["known"]);
    assert_eq!(rows[0].kind, "member_access");
}

#[test]
fn caller_search_skips_builder_scope_for_helpers_without_class_returns() {
    const CALLERS: usize = 2_048;
    let call_sites = (0..CALLERS)
        .map(|index| {
            format!("def caller_{index}():\n    value = make_value()\n    return value.get()\n\n")
        })
        .collect::<String>();
    let callers = format!("from pkg.builders import make_value\n\n{call_sites}");
    let (_repo, db) = scanned_repo(&[
        (
            "pkg/widget.py",
            "class Widget:\n    def get(self):\n        pass\n",
        ),
        (
            "pkg/builders.py",
            "def make_value():\n    return object()\n",
        ),
        ("src/callers.py", &callers),
    ]);
    let conn = open_read_write(&db).unwrap();
    let pending_calls: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM pending_relationships WHERE target_terminal_name = 'get'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_calls, CALLERS as i64);
    conn.execute("DELETE FROM identifiers WHERE name = 'get'", [])
        .unwrap();
    drop(conn);

    let conn = open_read_only(&db).unwrap();
    let operations = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observed = operations.clone();
    conn.progress_handler(
        100,
        Some(move || observed.fetch_add(100, std::sync::atomic::Ordering::Relaxed) >= 1_250_000),
    )
    .unwrap();
    let result = find_references_scoped(&conn, "Widget.get", "callers", 20, false, None);
    conn.progress_handler(0, None::<fn() -> bool>).unwrap();
    let rows = result.expect("untyped builders should not trigger a full-scope check per caller");

    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn a_super_read_without_metadata_is_not_a_supported_reference_to_itself() {
    let (_repo, db) = scanned_repo(&[(
        "types.py",
        "class Base:\n    def convert(self):\n        return 1\n\nclass Paths(Base):\n    def convert(self):\n        base = super().convert\n        return base()\n",
    )]);
    let conn = open_read_write(&db).unwrap();
    conn.execute(
        "UPDATE identifiers SET metadata_json = NULL WHERE name = 'convert'",
        [],
    )
    .unwrap();
    let rows = find_references_scoped(&conn, "Paths.convert", "callers", 20, false, None).unwrap();
    assert!(!rows.is_empty());
    assert!(
        rows.iter()
            .all(|row| row.kind == "member_access (candidate)"),
        "{rows:?}"
    );
}

#[test]
fn a_css_selector_is_not_a_candidate_reference_to_a_method() {
    let (_repo, db) = scanned_repo(&[
        (
            "app.py",
            "class Scaffold:\n    def post(self):\n        pass\n",
        ),
        ("static/style.css", ".post > header {\n  color: red;\n}\n"),
    ]);
    let conn = open_read_only(&db).unwrap();
    let rows = find_references_scoped(&conn, "Scaffold.post", "callers", 20, false, None).unwrap();
    assert!(rows.is_empty(), "{rows:?}");
}

#[test]
fn member_access_with_missing_metadata_is_a_candidate() {
    let (_repo, db) = scanned_repo(&[(
        "runners.py",
        "class Runner:\n    def invoke(self):\n        pass\n\nclass Child(Runner):\n    def read(self):\n        return super().invoke\n",
    )]);
    let conn = open_read_write(&db).unwrap();
    for metadata in [None, Some("{")] {
        conn.execute(
            "UPDATE identifiers SET metadata_json = ?1 WHERE name = 'invoke'",
            [metadata],
        )
        .unwrap();
        let rows =
            find_references_scoped(&conn, "Runner.invoke", "callers", 20, false, None).unwrap();
        assert_eq!(caller_names(&rows), ["read"]);
        assert_eq!(rows[0].kind, "member_access (candidate)");
    }
}

#[test]
fn member_access_with_missing_metadata_never_reaches_a_non_callable_target() {
    let (_repo, db) = scanned_repo(&[(
        "counter.rs",
        "struct Counter { value: i32 }\nfn read(counter: &Counter) -> i32 { counter.value }\n",
    )]);
    let conn = open_read_write(&db).unwrap();
    for metadata in [None, Some("{")] {
        conn.execute(
            "UPDATE identifiers SET metadata_json = ?1 WHERE name = 'value'",
            [metadata],
        )
        .unwrap();
        let rows = find_references_scoped(&conn, "value", "callers", 20, false, None).unwrap();
        assert!(rows.is_empty(), "{rows:?}");
    }
}

#[test]
fn a_call_is_not_duplicated_as_a_candidate_member_access() {
    let (_repo, db) = scanned_repo(&[
        (
            "runner.py",
            "class Runner:\n    def invoke(self):\n        pass\n",
        ),
        (
            "client.py",
            "from runner import Runner\n\ndef run():\n    value = Runner()\n    value.invoke()\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let rows = find_references_scoped(&conn, "Runner.invoke", "callers", 20, false, None).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, "calls");
    assert_eq!(rows[0].from_symbol_name, "run");
}

#[test]
fn non_callable_member_access_keeps_its_kind_without_type_metadata() {
    let (_repo, db) = scanned_repo(&[(
        "counter.rs",
        "struct Counter { value: i32 }\nfn read(counter: &Counter) -> i32 { counter.value }\n",
    )]);
    let conn = open_read_write(&db).unwrap();
    conn.execute_batch("ALTER TABLE symbols RENAME COLUMN metadata_json TO legacy_metadata_json;")
        .unwrap();
    let rows = find_references_scoped(&conn, "value", "callers", 20, false, None).unwrap();
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].kind, "member_access");
}

#[test]
fn find_references_reports_type_usages_of_a_struct() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/config.rs",
            "pub struct Config {\n    pub path: String,\n}\n",
        ),
        (
            "src/loader.rs",
            "use crate::config::Config;\n\npub fn load(existing: &Config) -> String {\n    existing.path.clone()\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "Config", "callers", 20, false, None).unwrap();

    let type_usage = refs
        .iter()
        .find(|r| r.kind == "type_usage")
        .unwrap_or_else(|| panic!("expected a type_usage reference, got {refs:?}"));
    assert_eq!(type_usage.from_symbol_name, "load");
    assert_eq!(type_usage.path, "src/loader.rs");
    assert_eq!(type_usage.start_line, Some(3));
}

#[test]
fn name_only_type_usages_stay_with_the_declaring_nested_class_and_language() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "java/CustomTypeAdaptersTest.java",
            "package demo;\npublic class CustomTypeAdaptersTest {\n    public static class Foo {}\n    Foo value;\n    Foo use() { return new Foo(); }\n}\n",
        ),
        (
            "java/RawSerializationTest.java",
            "package demo;\npublic class RawSerializationTest {\n    public static class Foo {}\n    Foo use() { return new Foo(); }\n}\n",
        ),
        (
            "python/other.py",
            "class Foo:\n    pass\n\ndef use(value: Foo) -> Foo:\n    return Foo()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "Foo",
        "callers",
        20,
        false,
        Some("java/CustomTypeAdaptersTest.java"),
    )
    .unwrap();

    let mut sites: Vec<_> = refs
        .iter()
        .map(|row| (row.kind.as_str(), row.path.as_str()))
        .collect();
    sites.sort_unstable();
    assert_eq!(
        sites,
        [
            ("calls", "java/CustomTypeAdaptersTest.java"),
            ("type_usage", "java/CustomTypeAdaptersTest.java"),
        ],
        "{refs:?}"
    );
}

#[test]
fn member_access_identifiers_do_not_cross_language_boundaries() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/HTTPHeaders.java",
            "package demo;\npublic class HTTPHeaders {\n    public String header() { return \"\"; }\n}\n",
        ),
        (
            "src/reader.js",
            "export function read(headers) { return headers.header; }\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "HTTPHeaders.header",
        "callers",
        20,
        false,
        Some("src/HTTPHeaders.java"),
    )
    .unwrap();

    assert!(refs.is_empty(), "{refs:?}");
}

#[test]
fn name_only_type_usages_resolve_to_the_nearest_nested_type() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/Outer.java",
        "package demo;\nclass Outer {\n    static class Foo {}\n    Foo outerField;\n    static class Inner {\n        static class Foo {}\n        Foo field;\n        Foo use(Foo value) { return value; }\n    }\n}\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();
    let outer_foo_id: String = conn
        .query_row(
            "SELECT outer_foo.symbol_id FROM symbols outer_foo
             JOIN symbols outer_type ON outer_type.symbol_id = outer_foo.parent_symbol_id
             WHERE outer_foo.path = 'src/Outer.java'
               AND outer_foo.name = 'Foo' AND outer_foo.kind = 'class'
               AND outer_type.name = 'Outer'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let inner_foo_id: String = conn
        .query_row(
            "SELECT inner_foo.symbol_id FROM symbols inner_foo
             JOIN symbols inner_type ON inner_type.symbol_id = inner_foo.parent_symbol_id
             WHERE inner_foo.path = 'src/Outer.java'
               AND inner_foo.name = 'Foo' AND inner_foo.kind = 'class'
               AND inner_type.name = 'Inner'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs = find_references_for_symbol(&conn, "Foo", "callers", 20, &inner_foo_id).unwrap();

    let type_usages: Vec<_> = refs.iter().filter(|row| row.kind == "type_usage").collect();
    assert!(
        !type_usages.is_empty(),
        "the nearer Inner.Foo must retain its own type usages: {refs:?}"
    );
    assert!(
        type_usages.iter().all(|row| row.path == "src/Outer.java"),
        "{type_usages:?}"
    );

    let outer_refs =
        find_references_for_symbol(&conn, "Foo", "callers", 20, &outer_foo_id).unwrap();
    let outer_type_usages: Vec<_> = outer_refs
        .iter()
        .filter(|row| row.kind == "type_usage")
        .collect();
    assert_eq!(
        outer_type_usages
            .iter()
            .map(|row| row.start_line)
            .collect::<Vec<_>>(),
        [Some(4)],
        "Outer.Foo must not claim the nearer Inner.Foo usages: {outer_refs:?}"
    );
}

#[test]
fn name_only_type_usages_resolve_to_a_same_file_top_level_type_first() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "One.java",
            "package a;\nclass Foo {}\nclass One {\n    Foo field;\n    Foo use(Foo value) { return value; }\n}\n",
        ),
        (
            "Two.java",
            "package b;\nclass Foo {}\nclass Two {\n    Foo field;\n    Foo use(Foo value) { return value; }\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let one_foo_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols
             WHERE path = 'One.java' AND name = 'Foo' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs = find_references_for_symbol(&conn, "Foo", "callers", 20, &one_foo_id).unwrap();
    let type_usages: Vec<_> = refs.iter().filter(|row| row.kind == "type_usage").collect();

    assert!(!type_usages.is_empty(), "{refs:?}");
    assert!(
        type_usages.iter().all(|row| row.path == "One.java"),
        "same-named type usages in another package must not reach One.Foo: {type_usages:?}"
    );
}

#[test]
fn name_only_identifiers_allow_only_the_supported_qml_and_razor_language_bridges() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/BridgeWidget.cpp",
            "class BridgeWidget { public: int value; };\n",
        ),
        (
            "src/BridgeModel.cs",
            "public class BridgeModel { public string Title { get; set; } }\n",
        ),
        (
            "ui/View.qml",
            "import QtQuick\nItem {\n    property BridgeWidget widget\n    property int currentValue: widget.value\n}\n",
        ),
        (
            "ui/View.razor",
            "@inject BridgeModel Model\n<p>@Model.Title</p>\n",
        ),
        ("src/other.py", "def use(value):\n    return value\n"),
    ]);
    let conn = open_read_write(&db_path).unwrap();
    conn.execute_batch(
        r#"INSERT INTO reference_sites
            (reference_site_id, file_id, path, language, is_exact, provenance)
         SELECT 'rs_qml_bridge', file_id, path, language, 0, 'spanless'
         FROM files WHERE path = 'ui/View.qml';
         INSERT INTO reference_sites
            (reference_site_id, file_id, path, language, is_exact, provenance)
         SELECT 'rs_razor_bridge', file_id, path, language, 0, 'spanless'
         FROM files WHERE path = 'ui/View.razor';
         INSERT INTO reference_sites
            (reference_site_id, file_id, path, language, is_exact, provenance)
         SELECT 'rs_unrelated', file_id, path, language, 0, 'spanless'
         FROM files WHERE path = 'src/other.py';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_qml_bridge', 'rs_qml_bridge', file_id, path, language,
                'BridgeWidget', 'type_usage', 3, 13, 3, 24, 0, 11, 1.0, '{}'
         FROM files WHERE path = 'ui/View.qml';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_qml_member', 'rs_qml_bridge', file_id, path, language,
                'value', 'member_access', 4, 39, 4, 44, 0, 5, 1.0,
                '{"receiver":"widget"}'
         FROM files WHERE path = 'ui/View.qml';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_razor_bridge', 'rs_razor_bridge', file_id, path, language,
                'BridgeModel', 'type_usage', 1, 8, 1, 19, 0, 11, 1.0, '{}'
         FROM files WHERE path = 'ui/View.razor';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_razor_member', 'rs_razor_bridge', file_id, path, language,
                'Title', 'member_access', 2, 10, 2, 15, 0, 5, 1.0,
                '{"receiver":"Model"}'
         FROM files WHERE path = 'ui/View.razor';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_unrelated', 'rs_unrelated', file_id, path, language,
                'BridgeWidget', 'type_usage', 1, 0, 1, 12, 0, 12, 1.0, '{}'
         FROM files WHERE path = 'src/other.py';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_unrelated_member', 'rs_unrelated', file_id, path, language,
                'value', 'member_access', 1, 0, 1, 5, 0, 5, 1.0,
                '{"receiver":"other"}'
         FROM files WHERE path = 'src/other.py';"#,
    )
    .unwrap();

    let language_for = |path: &str| -> String {
        conn.query_row(
            "SELECT language FROM files WHERE path = ?1",
            [path],
            |row| row.get(0),
        )
        .unwrap()
    };
    assert_eq!(language_for("ui/View.qml"), "qml");
    assert_eq!(language_for("ui/View.razor"), "razor");
    assert_eq!(language_for("src/BridgeWidget.cpp"), "cpp");
    assert_eq!(language_for("src/BridgeModel.cs"), "csharp");

    let target_id = |path: &str, name: &str| -> String {
        conn.query_row(
            "SELECT symbol_id FROM symbols WHERE path = ?1 AND name = ?2 ORDER BY start_line LIMIT 1",
            [path, name],
            |row| row.get(0),
        )
        .unwrap()
    };
    let cpp_refs = find_references_for_symbol(
        &conn,
        "BridgeWidget",
        "callers",
        20,
        &target_id("src/BridgeWidget.cpp", "BridgeWidget"),
    )
    .unwrap();
    let csharp_refs = find_references_for_symbol(
        &conn,
        "BridgeModel",
        "callers",
        20,
        &target_id("src/BridgeModel.cs", "BridgeModel"),
    )
    .unwrap();
    let cpp_member_refs = find_references_for_symbol(
        &conn,
        "value",
        "callers",
        20,
        &target_id("src/BridgeWidget.cpp", "value"),
    )
    .unwrap();
    let csharp_member_refs = find_references_for_symbol(
        &conn,
        "Title",
        "callers",
        20,
        &target_id("src/BridgeModel.cs", "Title"),
    )
    .unwrap();

    assert!(
        cpp_refs
            .iter()
            .any(|row| row.path == "ui/View.qml" && row.kind == "type_usage"),
        "QML-to-C++ bridge was filtered: {cpp_refs:?}"
    );
    assert!(
        !cpp_refs.iter().any(|row| row.path == "src/other.py"),
        "unrelated-language type usage leaked through: {cpp_refs:?}"
    );
    assert!(
        csharp_refs
            .iter()
            .any(|row| row.path == "ui/View.razor" && row.kind == "type_usage"),
        "Razor-to-C# bridge was filtered: {csharp_refs:?}"
    );
    assert!(
        cpp_member_refs
            .iter()
            .any(|row| row.path == "ui/View.qml" && row.kind == "member_access"),
        "QML-to-C++ member access bridge was filtered: {cpp_member_refs:?}"
    );
    assert!(
        !cpp_member_refs.iter().any(|row| row.path == "src/other.py"),
        "unrelated-language member access leaked through: {cpp_member_refs:?}"
    );
    assert!(
        csharp_member_refs
            .iter()
            .any(|row| row.path == "ui/View.razor" && row.kind == "member_access"),
        "Razor-to-C# member access bridge was filtered: {csharp_member_refs:?}"
    );
}

#[test]
fn find_references_reports_member_accesses_of_a_field() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/config.rs",
            "pub struct Config {\n    pub path: String,\n}\n",
        ),
        (
            "src/loader.rs",
            "use crate::config::Config;\n\npub fn load(existing: &Config) -> String {\n    existing.path.clone()\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "path", "callers", 20, false, None).unwrap();

    let access = refs
        .iter()
        .find(|r| r.kind == "member_access")
        .unwrap_or_else(|| panic!("expected a member_access reference, got {refs:?}"));
    assert_eq!(access.from_symbol_name, "load");
    assert_eq!(access.start_line, Some(4));
}

fn caller_names(refs: &[code_kb_core::ReferenceSite]) -> Vec<String> {
    let mut names: Vec<String> = refs.iter().map(|r| r.from_symbol_name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

#[test]
fn callers_of_a_nested_csharp_static_method_follow_the_outer_class_across_files() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/Defs.cs",
            "public class Outer\n{\n    public static class Inner\n    {\n        public static int Chain()\n        {\n            return 1;\n        }\n    }\n}\n",
        ),
        (
            "src/Caller.cs",
            "public class Caller\n{\n    public int Call()\n    {\n        return Outer.Inner.Chain();\n    }\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "Chain", "callers", 20, false, None).unwrap();

    assert_eq!(caller_names(&refs), vec!["Call"], "got {refs:?}");
    assert_eq!(refs[0].kind, "calls");
    assert_eq!(refs[0].path, "src/Caller.cs");
    assert_eq!(refs[0].start_line, Some(5));
}

#[test]
fn callers_of_a_nested_scala_method_follow_the_outer_object_in_another_file() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/Defs.scala",
            "object Outer {\n  object Inner {\n    def chain(): Int = 1\n  }\n}\n",
        ),
        (
            "src/Caller.scala",
            "object Caller {\n  def caller(): Int = Outer.Inner.chain()\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "chain", "callers", 20, false, None).unwrap();

    assert_eq!(caller_names(&refs), vec!["caller"], "got {refs:?}");
    assert_eq!(refs[0].kind, "calls");
    assert_eq!(refs[0].path, "src/Caller.scala");
    assert_eq!(refs[0].start_line, Some(2));
}

const REPOSITORY_CS: &str = "using System;
using System.Collections.Generic;

namespace CIMProfile.Core.Repositories
{
    public class EmailSettingQueryRepository
    {
        public static class EmailSettingNames
        {
            public static readonly string AnnualAttestationNotification = \"AnnualAttestationNotification\";
        }

        public static class EmailSettingPlaceholderGenerators
        {
            public static Dictionary<string, string> AnnualAttestationNotification(string dashboardUrl, string unitHeadName, DateTime deadline, IEnumerable<string> cims)
            {
                return new Dictionary<string, string>();
            }
        }
    }
}
";

const BUILDER_CS: &str = "using System;
using System.Collections.Generic;
using CIMProfile.Core.Repositories;

namespace CIMProfile.Core.ApplicationServices
{
    public class AnnualReportingAttestationNotificationBuilder
    {
        public string Build(string recipientEmail, string unitHeadName, DateTime deadline, IEnumerable<string> cims)
        {
            var emailId = EmailSettingQueryRepository.EmailSettingNames.AnnualAttestationNotification;
            var body = EmailSettingService.GenerateTemplatedEmailBody(\"x\", EmailSettingQueryRepository.EmailSettingPlaceholderGenerators.AnnualAttestationNotification(\"url\", unitHeadName, deadline, cims));
            return emailId + body;
        }
    }
}
";

const CONTROLLER_CS: &str = "using System;
using CIMProfile.Core.Repositories;

namespace CIMProfile.Web.Controllers
{
    public class EmailSettingController
    {
        public object GetEmailSettingPreview()
        {
            var placeholders = EmailSettingQueryRepository.EmailSettingPlaceholderGenerators.AnnualAttestationNotification(\"https://iu.edu\", \"Jane Smith\", DateTime.Now, new[] { \"a\" });
            return placeholders;
        }
    }
}
";

const TEST_CS: &str = "using System;
using CIMProfile.Core.Repositories;

namespace CIMProfile.Tests
{
    public class ReportingAttestationServiceShould
    {
        public void Sends()
        {
            var today = DateTime.Now;
            var body1 = EmailSettingService.GenerateTemplatedEmailBody(\"x\", EmailSettingQueryRepository.EmailSettingPlaceholderGenerators.AnnualAttestationNotification(\"\", \"\", today, new[] { \"CIM 1\" }));
        }
    }
}
";

#[test]
fn same_named_constant_and_method_in_sibling_nested_classes_keep_separate_callers() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "Core/Repositories/EmailSettingQueryRepository.cs",
            REPOSITORY_CS,
        ),
        (
            "Core/ApplicationServices/AnnualReportingAttestationNotificationBuilder.cs",
            BUILDER_CS,
        ),
        ("Web/Controllers/EmailSettingController.cs", CONTROLLER_CS),
        ("Tests/ReportingAttestationServiceShould.cs", TEST_CS),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let method_refs = find_references_scoped(
        &conn,
        "EmailSettingPlaceholderGenerators.AnnualAttestationNotification",
        "callers",
        20,
        false,
        None,
    )
    .unwrap();
    let mut method_sites: Vec<(String, String, Option<usize>, String)> = method_refs
        .iter()
        .map(|r| {
            (
                r.from_symbol_name.clone(),
                r.path.clone(),
                r.start_line,
                r.kind.clone(),
            )
        })
        .collect();
    method_sites.sort();
    assert_eq!(
        method_sites,
        vec![
            (
                "Build".to_string(),
                "Core/ApplicationServices/AnnualReportingAttestationNotificationBuilder.cs"
                    .to_string(),
                Some(12),
                "calls".to_string()
            ),
            (
                "GetEmailSettingPreview".to_string(),
                "Web/Controllers/EmailSettingController.cs".to_string(),
                Some(10),
                "calls".to_string()
            ),
            (
                "Sends".to_string(),
                "Tests/ReportingAttestationServiceShould.cs".to_string(),
                Some(11),
                "calls".to_string()
            ),
        ]
    );

    let constant_refs = find_references_scoped(
        &conn,
        "EmailSettingNames.AnnualAttestationNotification",
        "callers",
        20,
        false,
        None,
    )
    .unwrap();
    assert_eq!(constant_refs.len(), 1, "got {constant_refs:?}");
    assert_eq!(constant_refs[0].from_symbol_name, "Build");
    assert_eq!(constant_refs[0].kind, "member_access");
    assert_eq!(
        constant_refs[0].path,
        "Core/ApplicationServices/AnnualReportingAttestationNotificationBuilder.cs"
    );
    assert_eq!(constant_refs[0].start_line, Some(11));
}

#[test]
fn same_named_containers_with_same_named_members_keep_the_shared_caller() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/A.cs",
            "namespace A\n{\n    public static class Settings\n    {\n        public static readonly int Value = 1;\n    }\n}\n",
        ),
        (
            "src/B.cs",
            "namespace B\n{\n    public static class Settings\n    {\n        public static readonly int Value = 2;\n    }\n}\n",
        ),
        (
            "src/Reader.cs",
            "public class Reader\n{\n    public int Read()\n    {\n        return Settings.Value;\n    }\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    for declaring_file in ["src/A.cs", "src/B.cs"] {
        let refs =
            find_references_scoped(&conn, "Value", "callers", 20, false, Some(declaring_file))
                .unwrap();
        assert_eq!(
            caller_names(&refs),
            vec!["Read"],
            "{declaring_file}: got {refs:?}"
        );
        assert_eq!(refs[0].kind, "member_access");
        assert_eq!(refs[0].start_line, Some(5));
    }
}

#[test]
fn references_and_blast_radius_report_the_same_missing_symbol() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/config.rs",
        "pub struct Config {\n    pub path: String,\n}\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();

    let refs_error = find_references_scoped(&conn, "Confgi", "callers", 20, false, None)
        .unwrap_err()
        .to_string();
    let blast_error = compute_blast_radius(&conn, &["Confgi"], &[], 1, 20)
        .unwrap_err()
        .to_string();

    assert_eq!(refs_error, blast_error);
    assert!(refs_error.contains("Did you mean one of:"), "{refs_error}");
    assert!(refs_error.contains("`Config`"), "{refs_error}");
}

const QML_SINGLETON_AND_CONSUMER: &[(&str, &str)] = &[
    (
        "Commons/Color.qml",
        "pragma Singleton\nimport QtQuick\n\nQtObject {\n    property color foreground: \"#ffffff\"\n    property color background: \"#000000\"\n}\n",
    ),
    (
        "Ui/Button.qml",
        "import QtQuick\nimport \"../Commons\"\n\nRectangle {\n    color: Color.background\n    border.color: Color.foreground\n}\n",
    ),
];

const QUALIFIED_RECEIVER_JS: &[(&str, &str)] = &[
    (
        "src/runtime.js",
        "export class runtime {\n    static start() {\n        return 1;\n    }\n}\n",
    ),
    (
        "src/host.js",
        "export function boot(chrome) {\n    return chrome.runtime.lastError;\n}\n\nexport function version() {\n    return runtime.tag;\n}\n",
    ),
];

#[test]
fn callers_of_a_type_include_the_member_accesses_that_name_it_as_receiver() {
    let (_repo, db_path) = scanned_repo(QML_SINGLETON_AND_CONSUMER);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "Color",
        "callers",
        20,
        false,
        Some("Commons/Color.qml"),
    )
    .unwrap();

    type Site = (String, Option<usize>, String, String, Option<usize>);
    let mut sites: Vec<Site> = refs
        .iter()
        .map(|r| {
            (
                r.path.clone(),
                r.start_line,
                r.to_symbol_name.clone(),
                r.kind.clone(),
                r.occurrences,
            )
        })
        .collect();
    sites.sort();
    assert_eq!(
        sites,
        vec![(
            "Ui/Button.qml".to_string(),
            Some(5),
            "background".to_string(),
            "member_access".to_string(),
            Some(2)
        )],
        "{refs:?}"
    );
}

#[test]
fn a_call_site_caller_row_reports_no_occurrence_count() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "Core/Repositories/EmailSettingQueryRepository.cs",
            REPOSITORY_CS,
        ),
        (
            "Core/ApplicationServices/AnnualReportingAttestationNotificationBuilder.cs",
            BUILDER_CS,
        ),
        ("Web/Controllers/EmailSettingController.cs", CONTROLLER_CS),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "EmailSettingPlaceholderGenerators.AnnualAttestationNotification",
        "callers",
        20,
        false,
        None,
    )
    .unwrap();

    assert!(refs.iter().any(|r| r.kind == "calls"), "{refs:?}");
    assert!(refs.iter().all(|r| r.occurrences.is_none()), "{refs:?}");
}

#[test]
fn a_member_access_under_a_foreign_qualifier_is_not_a_reference_to_the_type() {
    let (_repo, db_path) = scanned_repo(QUALIFIED_RECEIVER_JS);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "runtime",
        "callers",
        20,
        false,
        Some("src/runtime.js"),
    )
    .unwrap();

    assert!(
        !refs.iter().any(|r| r.to_symbol_name == "lastError"),
        "{refs:?}"
    );
}

#[test]
fn receiver_matches_of_a_type_come_after_the_rows_that_name_it() {
    let (_repo, db_path) = scanned_repo(QUALIFIED_RECEIVER_JS);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "runtime",
        "callers",
        20,
        false,
        Some("src/runtime.js"),
    )
    .unwrap();

    let names: Vec<&str> = refs.iter().map(|r| r.to_symbol_name.as_str()).collect();

    assert_eq!(names, vec!["runtime", "tag"], "{refs:?}");
}

const QML_SIGNAL_AND_HANDLERS: &[(&str, &str)] = &[
    (
        "Ui/Button.qml",
        "import QtQuick\n\nItem {\n    signal clicked()\n}\n",
    ),
    (
        "App/Main.qml",
        "import QtQuick\nimport \"../Ui\"\n\nItem {\n    Button {\n        id: button\n    }\n}\n",
    ),
];

#[test]
fn a_signal_handler_is_a_candidate_until_its_receiver_names_the_owner() {
    let (_repo, db_path) = scanned_repo(QML_SIGNAL_AND_HANDLERS);
    let conn = open_read_write(&db_path).unwrap();
    conn.execute_batch(
        r#"INSERT INTO reference_sites
            (reference_site_id, file_id, path, language, is_exact, provenance)
         SELECT 'rs_handlers', file_id, path, language, 0, 'spanless'
         FROM files WHERE path = 'App/Main.qml';
         INSERT INTO identifiers
            (identifier_id, reference_site_id, file_id, path, language, name, kind,
             start_line, start_column, end_line, end_column, start_byte, end_byte,
             confidence, metadata_json)
         SELECT 'i_' || v.id, 'rs_handlers', f.file_id, f.path, f.language,
                'clicked', 'member_access', v.line, 0, v.line, 9, 0, 9, 1.0, v.metadata
         FROM files f
         JOIN (SELECT 'owned' AS id, 6 AS line,
                      '{"role":"signal_handler","receiver":"Button"}' AS metadata
               UNION ALL SELECT 'unowned', 7, '{"role":"signal_handler"}'
               UNION ALL SELECT 'foreign', 8,
                      '{"role":"signal_handler","receiver":"Timer"}'
               UNION ALL SELECT 'plain', 9, '{"receiver":"Button"}') v
         WHERE f.path = 'App/Main.qml'"#,
    )
    .unwrap();

    let refs = find_references_scoped(
        &conn,
        "clicked",
        "callers",
        20,
        false,
        Some("Ui/Button.qml"),
    )
    .unwrap();

    let mut sites: Vec<(Option<usize>, &str)> = refs
        .iter()
        .map(|r| (r.start_line, r.kind.as_str()))
        .collect();
    sites.sort();
    assert_eq!(
        sites,
        vec![
            (Some(6), "handler"),
            (Some(7), "handler (candidate)"),
            (Some(8), "handler (candidate)"),
            (Some(9), "member_access"),
        ],
        "{refs:?}"
    );
}

const INHERITANCE_SITES_THAT_ALSO_EMIT_IDENTIFIERS: &[(&str, &str)] = &[
    (
        "src/tree.py",
        "class Parent:\n    def go(self):\n        return 1\n\n\nclass Child(Parent):\n    def go(self):\n        return 2\n",
    ),
    (
        "src/base.ts",
        "export class Base {\n    go() { return 1; }\n}\n",
    ),
    (
        "src/leaf.ts",
        "import { Base } from \"./base\";\n\nexport class Leaf extends Base {\n    go() { return 2; }\n}\n",
    ),
];

#[test]
fn a_site_with_both_a_relationship_and_an_identifier_yields_one_row() {
    let (_repo, db_path) = scanned_repo(INHERITANCE_SITES_THAT_ALSO_EMIT_IDENTIFIERS);
    let conn = open_read_only(&db_path).unwrap();

    let sites = |refs: &[code_kb_core::ReferenceSite]| -> Vec<(String, Option<usize>, String)> {
        refs.iter()
            .map(|r| (r.path.clone(), r.start_line, r.kind.clone()))
            .collect()
    };

    let concrete =
        find_references_scoped(&conn, "Parent", "callers", 20, false, Some("src/tree.py")).unwrap();
    let pending =
        find_references_scoped(&conn, "Base", "callers", 20, false, Some("src/base.ts")).unwrap();

    assert_eq!(
        sites(&concrete),
        vec![("src/tree.py".to_string(), Some(6), "extends".to_string())],
        "{concrete:?}"
    );
    assert_eq!(
        sites(&pending),
        vec![("src/leaf.ts".to_string(), Some(3), "extends".to_string())],
        "{pending:?}"
    );
}

#[test]
fn a_python_module_receiver_matches_its_package_and_a_local_class_shadows() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/pkg/__init__.py", "from .app import App, make\n"),
        (
            "src/pkg/app.py",
            "class App:\n    pass\n\n\ndef make():\n    return 1\n",
        ),
        (
            "tests/test_app.py",
            "import pkg\nfrom other import helpers\n\n\ndef test_module_call():\n    return pkg.App()\n\n\ndef test_local_class():\n    class App(pkg.App):\n        pass\n\n    return App()\n\n\ndef test_other_module():\n    return helpers.make()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let callers = |name: &str| -> Vec<String> {
        let refs =
            find_references_scoped(&conn, name, "callers", 20, false, Some("src/pkg/app.py"))
                .unwrap();
        let calls: Vec<_> = refs.into_iter().filter(|r| r.kind == "calls").collect();
        caller_names(&calls)
    };

    assert_eq!(callers("App"), vec!["test_module_call"]);
    assert!(callers("make").is_empty());
}

#[test]
fn a_second_definition_in_the_callers_own_file_does_not_hide_the_target() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/twice.py",
            "def get():\n    return 1\n\n\ndef get():\n    return 2\n\n\ndef use():\n    return get()\n",
        ),
        (
            "src/issues.ts",
            "export type Base = { code: string };\n\nexport interface Issue extends Base {\n  expected: string;\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let callees = |name: &str, path: &str| -> Vec<String> {
        find_references_scoped(&conn, name, "callees", 20, false, Some(path))
            .unwrap()
            .into_iter()
            .map(|r| r.to_symbol_name)
            .collect()
    };

    assert!(callees("use", "src/twice.py").contains(&"get".to_string()));
    assert_eq!(callees("Issue", "src/issues.ts"), vec!["Base"]);
}

#[test]
fn a_self_call_reaches_a_method_inherited_from_a_base_in_another_file() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/base.py",
            "class App:\n    def trap(self, e):\n        return False\n",
        ),
        (
            "src/app.py",
            "from base import App\n\n\nclass Flask(App):\n    def handle(self, e):\n        return self.trap(e)\n",
        ),
        (
            "src/other.py",
            "class Other:\n    def trap(self, e):\n        return True\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let callees =
        find_references_scoped(&conn, "handle", "callees", 20, false, Some("src/app.py")).unwrap();
    let callers =
        find_references_scoped(&conn, "trap", "callers", 20, false, Some("src/base.py")).unwrap();
    let other_callers =
        find_references_scoped(&conn, "trap", "callers", 20, false, Some("src/other.py")).unwrap();

    assert_eq!(
        callees
            .iter()
            .map(|r| r.to_symbol_name.as_str())
            .collect::<Vec<_>>(),
        vec!["trap"]
    );
    assert_eq!(caller_names(&callers), vec!["handle"]);
    assert!(other_callers.is_empty(), "{other_callers:?}");
}

#[test]
fn blast_radius_follows_a_fixture_to_the_tests_that_take_it() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/web/app.py", "def report(e):\n    return str(e)\n"),
        (
            "tests/test_runner.py",
            "import pytest\nfrom web.app import report\n\n\n@pytest.fixture\ndef invoke():\n    return report(None)\n\n\ndef test_invoke_path(invoke):\n    assert invoke\n\n\nclass TestRoutes:\n    @pytest.fixture\n    def method_invoke(self):\n        return report(None)\n\n    def setup_method(self):\n        report(None)\n\n    def test_simple(self, method_invoke):\n        assert method_invoke\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["report"], &[], 2, 20).unwrap();

    let reasons: Vec<(&str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.reason.as_str()))
        .collect();
    assert!(
        reasons.contains(&("TestRoutes::test_simple", "uses fixture `method_invoke`")),
        "{reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|(_, reason)| reason.starts_with("fixture") || reason.starts_with("setup")),
        "{reasons:?}"
    );
    assert!(
        reasons.contains(&("test_invoke_path", "uses fixture `invoke`")),
        "{reasons:?}"
    );
}

#[test]
fn blast_radius_labels_a_test_class_setup_and_adds_the_tests_it_runs_before() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/Store.cs",
            "namespace App;\n\npublic static class Store\n{\n    public static string PathFor(string root) => root;\n}\n",
        ),
        (
            "tests/PathTests.cs",
            "using Xunit;\n\nnamespace App.Tests;\n\npublic sealed class PathTests\n{\n    private readonly string path;\n\n    public PathTests()\n    {\n        path = Store.PathFor(\"root\");\n    }\n\n    [Fact]\n    public void Opens()\n    {\n        Assert.NotNull(path);\n    }\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["PathFor"], &[], 2, 20).unwrap();

    let reasons: Vec<(&str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.reason.as_str()))
        .collect();
    assert!(
        reasons.contains(&("PathTests::Opens", "setup `PathTests` runs before it")),
        "{reasons:?}"
    );
    assert!(
        !reasons
            .iter()
            .any(|(name, _)| *name == "PathTests::PathTests"),
        "{reasons:?}"
    );
}

#[test]
fn blast_radius_reaches_tests_that_build_a_class_whose_call_method_reaches_the_target() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/app.py",
            "class App:\n    def __call__(self, environ):\n        return self.dispatch(environ)\n\n    def dispatch(self, environ):\n        return self.handle_user_exception(environ)\n\n    def handle_user_exception(self, e):\n        return e\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.app import App\n\n\n@pytest.fixture\ndef app():\n    return App()\n\n\n@pytest.fixture\ndef client(app):\n    return app.test_client()\n",
        ),
        (
            "tests/test_user_error_handler.py",
            "def test_user_error_is_handled(app, client):\n    assert client.get('/')\n\n\ndef test_user_exception_logger_is_set(app):\n    assert app.logger\n",
        ),
        (
            "tests/test_views.py",
            "from web.app import App\n\n\ndef test_view_renders():\n    assert App().test_client()\n\n\ndef test_exception_handling_renders():\n    assert App().test_client().get('/')\n",
        ),
        (
            "tests/test_config.py",
            "from web.app import App\n\n\ndef test_config_loads():\n    assert App()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["handle_user_exception"], &[], 3, 20).unwrap();

    let names: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    let position = |name: &str| {
        names
            .iter()
            .position(|n| *n == name)
            .unwrap_or_else(|| panic!("{name} missing from {:?}", result.likely_tests))
    };
    assert!(position("test_user_error_is_handled") < position("test_exception_handling_renders"));
    assert!(!names.contains(&"test_view_renders"), "{names:?}");
    assert!(!names.contains(&"test_config_loads"), "{names:?}");
    assert!(
        !names.contains(&"test_user_exception_logger_is_set"),
        "{names:?}"
    );
    let reason = &result.likely_tests[position("test_exception_handling_renders")].reason;
    assert!(
        reason.starts_with(
            "possible: builds `App`, and a test client calls its `__call__`, which can reach the target; shares"
        ),
        "{reason}"
    );
    let reason = &result.likely_tests[position("test_user_error_is_handled")].reason;
    assert!(
        reason.starts_with(
            "possible: builds `App` through fixture `app`, and a test client calls its `__call__`, which can reach the target; shares"
        ),
        "{reason}"
    );
}

#[test]
fn blast_radius_rendering_keeps_handler_priority_across_file_groups_with_a_limit() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/app.py",
            "class Flask:\n    def __call__(self, environ):\n        return self.handle_user_exception(environ)\n\n    def handle_user_exception(self, error):\n        return error\n\n    def errorhandler(self, error):\n        return error\n\n    def test_client(self):\n        return None\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.app import Flask\n\n\n@pytest.fixture\ndef app():\n    return Flask()\n",
        ),
        (
            "tests/test_basic.py",
            "def test_error_handling(app):\n    app.errorhandler(Exception)\n    app.test_client().get('/')\n\n\ndef test_teardown_request_handler(app):\n    app.test_client().get('/')\n",
        ),
        (
            "tests/test_regression.py",
            "def test_aborting(app):\n    abort(403)\n    app.test_client().get('/')\n",
        ),
        (
            "tests/test_extra.py",
            "def test_exception_logging(app):\n    app.test_client().get('/')\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["handle_user_exception"], &[], 3, 3).unwrap();
    assert_eq!(result.likely_tests.len(), 3, "{:?}", result.likely_tests);
    assert!(result.likely_tests_truncated, "{:?}", result.likely_tests);

    let rendered = format_blast_radius(&result);
    let handler = rendered.find("`test_error_handling`").unwrap();
    let aborting = rendered.find("`test_aborting`").unwrap();
    let name_only = rendered.find("`test_teardown_request_handler`").unwrap();
    assert!(handler < aborting && aborting < name_only, "{rendered}");
    assert!(!rendered.contains("`test_exception_logging`"), "{rendered}");
}

#[test]
fn blast_radius_lists_the_tests_behind_a_fixture_that_takes_a_fixture_not_the_fixture() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/web/app.py", "def report(e):\n    return str(e)\n"),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.app import report\n\n\n@pytest.fixture\ndef invoke():\n    return report(None)\n",
        ),
        (
            "tests/test_runner.py",
            "import pytest\n\n\nclass TestRunner:\n    @pytest.fixture\n    def runner(self, invoke):\n        return invoke\n\n    def test_runs(self, runner):\n        assert runner\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["report"], &[], 2, 20).unwrap();

    let reasons: Vec<(&str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.name.as_str(), t.reason.as_str()))
        .collect();
    assert!(
        reasons.contains(&("TestRunner::test_runs", "uses fixture `invoke`")),
        "{reasons:?}"
    );
    assert!(
        !reasons.iter().any(|(name, _)| name.ends_with("runner")),
        "{reasons:?}"
    );
}

#[test]
fn two_accesses_on_one_line_are_one_reference() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/app.py",
            "class App:\n    def wsgi_app(self, environ):\n        return environ\n",
        ),
        (
            "tests/test_app.py",
            "from app import App\n\n\ndef test_wrap(app: App):\n    app.wsgi_app = Wrap(app.wsgi_app)\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "wsgi_app", "callers", 20, false, None).unwrap();

    let wraps: Vec<_> = refs
        .iter()
        .filter(|r| r.from_symbol_name == "test_wrap")
        .collect();
    assert_eq!(wraps.len(), 1, "{refs:?}");
    assert_eq!(wraps[0].occurrences, None, "{refs:?}");

    let target = code_kb_core::get_symbol_by_name(&conn, "wsgi_app", Some("src/app.py"))
        .unwrap()
        .unwrap();
    let related = code_kb_core::find_related_tests(&conn, &target, 5).unwrap();
    assert!(related.iter().any(|t| t.name == "test_wrap"), "{related:?}");
}

#[test]
fn a_receiver_named_for_a_fixture_calls_methods_of_the_class_the_fixture_builds() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/base.py",
            "class Scaffold:\n    def route(self, rule):\n        return rule\n",
        ),
        (
            "src/web/app.py",
            "from web.base import Scaffold\n\n\nclass Flask(Scaffold):\n    def run(self):\n        return 1\n",
        ),
        (
            "src/web/other.py",
            "class Server:\n    def run(self):\n        return 2\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\nfrom web.app import Flask\n\n\n@pytest.fixture\ndef app():\n    return Flask()\n",
        ),
        (
            "tests/test_run.py",
            "def test_run(app):\n    app.run()\n    app.route('/')\n\n\ndef test_other(server):\n    server.run()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let callers = |name: &str, path: &str| -> Vec<String> {
        let refs = find_references_scoped(&conn, name, "callers", 20, false, Some(path)).unwrap();
        caller_names(&refs)
    };

    assert_eq!(callers("run", "src/web/app.py"), vec!["test_run"]);
    assert_eq!(callers("route", "src/web/base.py"), vec!["test_run"]);
    assert!(callers("run", "src/web/other.py").is_empty());
}

#[test]
fn a_receiver_built_by_a_call_calls_methods_of_the_class_that_call_returns() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/testing.py",
            "class CliRunner:\n    def invoke(self, args):\n        return args\n",
        ),
        (
            "src/web/other.py",
            "class Shell:\n    def invoke(self, args):\n        return args\n",
        ),
        (
            "src/web/app.py",
            "from web.testing import CliRunner\n\n\nclass Flask:\n    def test_cli_runner(self) -> CliRunner:\n        return CliRunner()\n",
        ),
        (
            "tests/conftest.py",
            "import pytest\n\n\n@pytest.fixture\ndef runner(app):\n    return app.test_cli_runner()\n",
        ),
        (
            "examples/tests/conftest.py",
            "import pytest\nfrom web.other import Shell\n\n\n@pytest.fixture\ndef runner():\n    return Shell()\n",
        ),
        (
            "tests/test_cli.py",
            "from web.testing import CliRunner\n\n\ndef test_local(app):\n    cli = app.test_cli_runner()\n    cli.invoke(['hello'])\n\n\ndef test_fixture(runner):\n    runner.invoke(['hello'])\n\n\ndef test_built(app):\n    made = CliRunner()\n    made.invoke(['hello'])\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let callers = |path: &str| -> Vec<String> {
        let refs =
            find_references_scoped(&conn, "invoke", "callers", 20, false, Some(path)).unwrap();
        caller_names(&refs)
    };

    assert_eq!(
        callers("src/web/testing.py"),
        vec!["test_built", "test_fixture", "test_local"]
    );
    assert!(callers("src/web/other.py").is_empty());
}

#[test]
fn callees_include_a_self_call_to_a_class_attribute() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/web/app.py",
        "class App:\n    should_ignore: None = None\n\n    def run(self, error):\n        limit = 3\n        if self.should_ignore(error):\n            return limit\n        return self.step()\n\n    def step(self):\n        return 1\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(&conn, "run", "callees", 20, false, None).unwrap();

    let mut callees: Vec<&str> = refs.iter().map(|r| r.to_symbol_name.as_str()).collect();
    callees.sort();
    assert_eq!(callees, vec!["should_ignore", "step"]);
}

#[test]
fn the_callers_of_a_constructor_are_the_calls_that_build_its_class() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/cli.py",
            "class Group:\n    def __init__(self):\n        self.name = 'cli'\n",
        ),
        (
            "src/web/app.py",
            "from web.cli import Group\n\n\nclass Flask:\n    def __init__(self):\n        self.cli = Group()\n\n\ndef annotate(app: Flask) -> Flask:\n    return app\n",
        ),
        (
            "tests/test_app.py",
            "from web.app import Flask\n\n\ndef test_builds():\n    assert Flask()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let refs = find_references_scoped(
        &conn,
        "Flask.__init__",
        "callers",
        20,
        false,
        Some("src/web/app.py"),
    )
    .unwrap();
    assert_eq!(caller_names(&refs), vec!["test_builds"], "{refs:?}");

    let result = compute_blast_radius(&conn, &["Group.__init__"], &[], 3, 20).unwrap();
    let tests: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(tests.contains(&"test_builds"), "{tests:?}");
}

#[test]
fn whole_file_blast_radius_ranks_constructor_only_tests_after_direct_callers() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/flask/cli.py",
            "class AppGroup:\n    def __init__(self):\n        pass\n\n\ndef invoke_cli():\n    return 1\n",
        ),
        (
            "src/flask/app.py",
            "from . import cli\n\n\nclass Flask:\n    def __init__(self):\n        self.cli = cli.AppGroup()\n",
        ),
        ("src/flask/__init__.py", "from .app import Flask\n"),
        (
            "src/flask/runner.py",
            "from flask.cli import invoke_cli\n\n\ndef run_cli():\n    return invoke_cli()\n",
        ),
        (
            "src/flask/entry.py",
            "from flask.runner import run_cli\n\n\ndef dispatch():\n    return run_cli()\n",
        ),
        (
            "tests/test_runner.py",
            "from flask.entry import dispatch\n\n\ndef test_runs_cli_command():\n    assert dispatch()\n",
        ),
        (
            "tests/test_application.py",
            "import flask\n\n\ndef test_constructs_app_only():\n    assert flask.Flask()\n\n\ndef test_constructs_another_app():\n    assert flask.Flask()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &[], &["src/flask/cli.py"], 4, 20).unwrap();
    let tests: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|test| test.name.as_str())
        .collect();
    let position = |name: &str| {
        tests
            .iter()
            .position(|test| *test == name)
            .unwrap_or_else(|| panic!("{name} missing from {tests:?}"))
    };

    let constructor_rows: Vec<_> = result
        .likely_tests
        .iter()
        .filter(|test| test.path == "tests/test_application.py")
        .collect();
    assert_eq!(constructor_rows.len(), 1, "{tests:?}");
    assert_eq!(constructor_rows[0].name, "tests/test_application.py");
    assert_eq!(
        constructor_rows[0].reason,
        "constructor-only callers (2 targets)"
    );
    assert!(
        position("test_runs_cli_command") < position("tests/test_application.py"),
        "{tests:?}"
    );
    let formatted = format_blast_radius(&result);
    assert!(
        formatted.contains("constructor-only callers (2 targets)"),
        "{formatted}"
    );
    assert!(
        !formatted.contains("`test_constructs_app_only`"),
        "{formatted}"
    );
}

#[test]
fn blast_radius_lists_tests_through_a_constructor_last_and_skips_app_factories() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/web/cli.py",
            "class Group:\n    def __init__(self):\n        self.name = 'cli'\n\n\ndef invoke_cli():\n    return 1\n",
        ),
        (
            "src/web/app.py",
            "from web.cli import Group\n\n\nclass Flask:\n    def __init__(self):\n        self.cli = Group()\n",
        ),
        (
            "tests/test_app.py",
            "from web.app import Flask\n\n\ndef test_builds():\n    assert Flask()\n",
        ),
        (
            "tests/test_apps/factory.py",
            "from web.app import Flask\n\n\ndef create_app():\n    return Flask()\n",
        ),
        (
            "src/web/runner.py",
            "from web.cli import invoke_cli\n\n\ndef run():\n    return invoke_cli()\n",
        ),
        (
            "tests/test_zz_run.py",
            "from web.runner import run\n\n\ndef test_zz_invoke():\n    assert run()\n\n\nclass TestRun:\n    def helper(self):\n        return run()\n\n    def test_uses_helper(self):\n        assert self.helper()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &[], &["src/web/cli.py"], 2, 20).unwrap();

    let tests: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(!tests.contains(&"create_app"), "{tests:?}");
    assert!(tests.contains(&"TestRun::helper"), "{tests:?}");
    let position = |name: &str| {
        tests
            .iter()
            .position(|t| *t == name)
            .unwrap_or_else(|| panic!("{name} missing: {tests:?}"))
    };
    assert!(
        position("test_zz_invoke") < position("test_builds"),
        "{tests:?}"
    );
}

#[test]
fn import_sites_list_imports_whose_module_holds_the_definition() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/pkg/app.py", "class App:\n    pass\n"),
        ("src/pkg/__init__.py", "from .app import App as App\n"),
        ("tests/test_app.py", "from pkg.app import App\n"),
        ("tests/test_other.py", "from other.widgets import App\n"),
        (
            "src/pkg/cli.py",
            "def load():\n    from . import App\n    return App\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let sites = code_kb_core::import_sites(&conn, "App", Some("src/pkg/app.py")).unwrap();

    let paths: Vec<&str> = sites.iter().map(|(path, _)| path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["src/pkg/__init__.py", "src/pkg/cli.py", "tests/test_app.py"]
    );
}

#[test]
fn a_member_access_on_an_unknown_type_or_to_a_private_module_function_is_not_a_reference() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/series.ts",
            "export class Series {\n  contains(x: number): boolean {\n    return x > 0;\n  }\n}\n\nconst error = () => 1;\n\nexport function run(): number {\n  return error();\n}\n",
        ),
        (
            "tests/series.test.ts",
            "import { Series } from '../src/series';\n\ntest('reads', () => {\n  const s = new Series();\n  const a = Assert.contains;\n  const b = s.contains;\n  const c = parsed.error;\n});\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let lines = |name: &str| -> Vec<(String, Option<usize>)> {
        find_references_scoped(&conn, name, "callers", 20, false, Some("src/series.ts"))
            .unwrap()
            .into_iter()
            .map(|r| (r.path, r.start_line))
            .collect()
    };

    let contains = lines("contains");
    assert!(
        contains.contains(&("tests/series.test.ts".to_string(), Some(6))),
        "{contains:?}"
    );
    assert!(
        !contains.contains(&("tests/series.test.ts".to_string(), Some(5))),
        "{contains:?}"
    );
    assert_eq!(
        lines("error"),
        vec![("src/series.ts".to_string(), Some(10))]
    );
}

#[test]
fn related_tests_leave_out_setup_methods_helper_classes_ambiguous_names_and_documents() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/calc.py",
            "class Calc:\n    def total(self):\n        return 1\n\n    def add(self):\n        return 2\n",
        ),
        ("src/other.py", "def add():\n    return 3\n"),
        (
            "docs/page.html",
            "<html><body><form></form></body></html>\n",
        ),
        (
            "tests/test_calc.py",
            "import unittest\nfrom calc import Calc\n\n\nclass TotalHolder:\n    pass\n\n\nclass CalcTest(unittest.TestCase):\n    def setUp(self):\n        Calc().total()\n\n    def test_total(self):\n        pass\n\n\ndef test_add_twice():\n    pass\n\n\ndef test_form():\n    pass\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = |name: &str, path: &str| -> Vec<String> {
        let target = code_kb_core::get_symbol_by_name(&conn, name, Some(path))
            .unwrap()
            .unwrap();
        code_kb_core::find_related_tests(&conn, &target, 10)
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect()
    };

    assert_eq!(related("total", "src/calc.py"), vec!["test_total"]);
    assert!(related("add", "src/calc.py").is_empty());
    assert!(related("form", "docs/page.html").is_empty());
}

#[test]
fn only_a_function_inside_the_body_of_another_function_counts_as_nested() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/view.js",
        "function View(name) {\n  this.name = name;\n}\n\nView.prototype.lookup = function lookup(name) {\n  return name;\n};\n\nfunction outer() {\n  function lookupLocal() {\n    return 1;\n  }\n  return lookupLocal();\n}\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();

    let rows =
        code_kb_core::fts_search_symbols_explained(&conn, "lookup", None, None, false, 20, true)
            .unwrap();
    let nested = |name: &str| {
        rows.iter()
            .find(|r| r.symbol.name == name)
            .and_then(|r| r.explain.as_ref())
            .map(|e| e.nested)
            .unwrap_or_else(|| panic!("{name} missing: {rows:?}"))
    };

    assert_eq!(nested("lookup"), 0.0);
    assert!(nested("lookupLocal") < 0.0);
}

#[test]
fn qualify_members_names_the_class_of_a_member_but_not_of_a_document_heading() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/cli.py",
            "class ScriptInfo:\n    def __init__(self):\n        pass\n\n\ndef main():\n    pass\n",
        ),
        ("docs/guide.md", "# Setup\n\n## Install\n\nText.\n"),
        (
            "lib/cart.dart",
            "class Cart {\n  final List<int> items;\n  const Cart.empty() : items = const [];\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let mut rows: Vec<_> = ["__init__", "main", "Install", "Cart.empty"]
        .iter()
        .map(|name| {
            code_kb_core::get_symbol_by_name(&conn, name, None)
                .unwrap()
                .unwrap()
        })
        .collect();

    code_kb_core::qualify_members(&conn, rows.iter_mut()).unwrap();

    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["ScriptInfo.__init__", "main", "Install", "Cart.empty"]
    );
}

#[test]
fn super_and_self_calls_reach_the_nearest_ancestor_that_defines_the_method() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/base.py",
            "class Scaffold:\n    def __init__(self):\n        pass\n\n    def add_rule(self):\n        pass\n\n\nclass App(Scaffold):\n    def __init__(self):\n        super().__init__()\n\n    def add_rule(self):\n        pass\n",
        ),
        (
            "src/app.py",
            "from base import App\n\n\nclass Flask(App):\n    def __init__(self):\n        super().__init__()\n        self.add_rule()\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let lines = |class: &str, name: &str| -> Vec<(String, Option<usize>)> {
        code_kb_core::find_references_for_symbol(
            &conn,
            name,
            "callers",
            20,
            &member_of(&conn, class, name),
        )
        .unwrap()
        .into_iter()
        .map(|site| (site.path, site.start_line))
        .collect()
    };

    assert_eq!(
        lines("App", "add_rule"),
        vec![("src/app.py".to_string(), Some(7))]
    );
    assert!(lines("Scaffold", "add_rule").is_empty());
    assert_eq!(
        lines("Scaffold", "__init__"),
        vec![("src/base.py".to_string(), Some(11))]
    );

    let app_init = code_kb_core::find_references_for_symbol(
        &conn,
        "__init__",
        "callers",
        20,
        &member_of(&conn, "App", "__init__"),
    )
    .unwrap();
    assert_eq!(
        app_init
            .iter()
            .map(|s| (s.path.as_str(), s.start_line))
            .collect::<Vec<_>>(),
        vec![("src/app.py", Some(6))]
    );
}

fn member_of(conn: &rusqlite::Connection, class: &str, name: &str) -> String {
    conn.query_row(
        "SELECT m.symbol_id FROM symbols m JOIN symbols c ON c.symbol_id = m.parent_symbol_id
         WHERE c.name = ?1 AND m.name = ?2",
        [class, name],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn a_constructor_gets_the_tests_that_build_its_class_even_one_defined_in_the_test() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/app.py",
            "class Flask:\n    def __init__(self, name):\n        self.name = name\n",
        ),
        (
            "tests/test_app.py",
            "from app import Flask\n\n\ndef test_builds():\n    Flask('x')\n\n\ndef test_local():\n    class Quiet(Flask):\n        pass\n\n    Quiet('y')\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let init = code_kb_core::get_symbol_by_id(&conn, &member_of(&conn, "Flask", "__init__"))
        .unwrap()
        .unwrap();

    let related: Vec<String> = code_kb_core::find_related_tests(&conn, &init, 5)
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(related, vec!["test_builds"]);

    let quiet = find_references_scoped(&conn, "Quiet", "callers", 20, false, None).unwrap();
    assert_eq!(caller_names(&quiet), vec!["test_local"]);
}

#[test]
fn a_callee_row_names_the_definition_it_reaches_or_the_candidates() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/a.py", "def helper():\n    return 1\n"),
        ("src/b.py", "def helper():\n    return 2\n"),
        (
            "src/jobs.py",
            "def drain():\n    return helper()\n\n\nclass Runner:\n    def run(self):\n        self.step()\n\n    def step(self):\n        return 3\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let drain = find_references_scoped(&conn, "drain", "callees", 20, false, None).unwrap();
    let run = find_references_scoped(&conn, "run", "callees", 20, false, None).unwrap();

    let helper = drain.iter().find(|r| r.to_symbol_name == "helper");
    assert_eq!(
        helper.and_then(|r| r.target.as_deref()),
        Some("one of `helper` (src/a.py:1), `helper` (src/b.py:1)"),
        "{drain:?}"
    );
    let step = run.iter().find(|r| r.to_symbol_name == "step");
    assert_eq!(
        step.and_then(|r| r.target.as_deref()),
        Some("`Runner.step` (src/jobs.py:9)"),
        "{run:?}"
    );
}

#[test]
fn a_subclass_write_to_an_attribute_its_base_defines_is_not_a_definition() {
    let (repo, db_path) = scanned_repo(&[
        (
            "src/base.py",
            "class Base:\n    label: str\n\n    def __init__(self):\n        self.ready = False\n\n    @property\n    def mode(self):\n        return 1\n",
        ),
        (
            "src/app.py",
            "from .base import Base\n\n\nclass App(Base):\n    def run(self):\n        self.ready = True\n        self.mode = 2\n        self.label = \"app\"\n        self.own = 3\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());

    let skeleton = file_skeleton_op(&workspace, &db_path, &conn, "src/app.py").unwrap();
    let ready = search_symbols_scoped(&conn, "ready", None, None, false, 20).unwrap();
    let mode = fts_search_symbols_scoped(&conn, "mode", None, None, false, 20).unwrap();

    assert!(!skeleton.contains("self.ready"), "{skeleton}");
    assert!(!skeleton.contains("self.mode"), "{skeleton}");
    assert!(skeleton.contains("self.label"), "{skeleton}");
    assert!(skeleton.contains("self.own"), "{skeleton}");
    let paths: Vec<_> = ready.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["src/base.py"]);
    assert!(
        mode.iter().all(|hit| hit.symbol.path == "src/base.py"),
        "{mode:?}"
    );
}

#[test]
fn a_whole_test_file_row_replaces_the_rows_of_the_tests_inside_it() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/store.py", "def path_for(root):\n    return root\n"),
        (
            "tests/test_store.py",
            "from store import path_for\n\n\ndef test_path():\n    assert path_for('r')\n",
        ),
        (
            "tests/test_other.py",
            "from store import path_for\n\n\ndef test_other_path():\n    assert path_for('r')\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let result = compute_blast_radius(&conn, &["path_for"], &[], 2, 20).unwrap();

    let rows: Vec<(&str, &str)> = result
        .likely_tests
        .iter()
        .map(|t| (t.path.as_str(), t.name.as_str()))
        .collect();
    assert_eq!(
        rows,
        [
            ("tests/test_store.py", "tests/test_store.py"),
            ("tests/test_other.py", "test_other_path"),
        ]
    );
}

#[test]
fn a_relative_typescript_import_with_a_js_extension_picks_the_file_it_names() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/v3/helpers/util.ts",
            "export namespace util {\n  export function joinValues(values: string[]): string {\n    return values.join(\"|\");\n  }\n}\n",
        ),
        (
            "src/v4/core/util.ts",
            "export function joinValues(values: string[]): string {\n  return values.join(\", \");\n}\n",
        ),
        (
            "src/v3/core/util.ts",
            "export function joinValues(values: string[]): string {\n  return values.join(\"/\");\n}\n",
        ),
        (
            "src/v3/locales/en.ts",
            "import { util } from \"../helpers/util.js\";\n\nexport function enMessage(values: string[]): string {\n  return util.joinValues(values);\n}\n",
        ),
        (
            "src/v4/locales/ru.ts",
            "import * as util from \"../core/util.js\";\n\nexport function ruMessage(values: string[]): string {\n  return util.joinValues(values);\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let callers = |file: &str| {
        caller_names(
            &find_references_scoped(&conn, "joinValues", "callers", 20, false, Some(file)).unwrap(),
        )
    };

    assert_eq!(callers("src/v4/core/util.ts"), vec!["ruMessage"]);
    assert_eq!(callers("src/v3/helpers/util.ts"), vec!["enMessage"]);
    assert!(callers("src/v3/core/util.ts").is_empty());
}

#[test]
fn an_attribute_write_stays_when_only_an_unrelated_class_of_the_base_name_defines_it() {
    let (repo, db_path) = scanned_repo(&[
        ("src/models/base.py", "class Base:\n    pass\n"),
        (
            "src/other/base.py",
            "class Base:\n    def __init__(self):\n        self.ready = False\n",
        ),
        (
            "src/app/app.py",
            "from models.base import Base\n\n\nclass App(Base):\n    def run(self):\n        self.ready = True\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let workspace = Workspace::new(repo.path().to_path_buf());

    let skeleton = file_skeleton_op(&workspace, &db_path, &conn, "src/app/app.py").unwrap();

    assert!(skeleton.contains("self.ready"), "{skeleton}");
}

#[test]
fn lookup_fills_its_limit_after_leaving_out_inherited_writes() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "src/base.py",
            "class Base:\n    def __init__(self):\n        self.ready = False\n",
        ),
        (
            "src/app.py",
            "from .base import Base\n\n\nclass App(Base):\n    def run(self):\n        self.ready = True\n",
        ),
        (
            "src/other.py",
            "class Other:\n    def __init__(self):\n        self.ready = 1\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let ready = search_symbols_scoped(&conn, "ready", None, None, false, 2).unwrap();

    let paths: Vec<_> = ready.iter().map(|s| s.path.as_str()).collect();
    assert_eq!(paths, ["src/base.py", "src/other.py"]);
}

#[test]
fn folded_imports_do_not_use_up_the_lookup_limit() {
    let (_repo, db_path) = scanned_repo(&[
        ("src/app.py", "class Flask:\n    pass\n"),
        ("src/a.py", "from app import Flask\n"),
        ("src/b.py", "from app import Flask\n"),
        ("src/c.py", "from app import Flask\n"),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let rows = search_symbols_scoped(&conn, "Flask", None, None, false, 1).unwrap();

    let kinds: Vec<_> = rows.iter().map(|s| s.kind.as_str()).collect();
    assert_eq!(kinds, ["class", "import", "import", "import"]);
}

#[test]
fn a_lookup_ending_in_a_dot_lists_the_members_of_that_class_in_source_order() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/app.py",
        "class App:\n    def run(self):\n        pass\n\n    def config(self):\n        pass\n\n\nclass Other:\n    def run(self):\n        pass\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();

    let members = search_symbols_scoped(&conn, "App.", None, None, false, 20).unwrap();

    let names: Vec<_> = members
        .iter()
        .map(|s| (s.name.as_str(), s.start_line))
        .collect();
    assert_eq!(names, [("run", 2), ("config", 5)]);
}

#[test]
fn a_member_lookup_skips_a_markdown_heading_with_the_class_name() {
    let (_repo, db_path) = scanned_repo(&[
        ("README.md", "# App\n\n## Install\n\nText.\n"),
        (
            "src/app.py",
            "class App:\n    def run(self):\n        pass\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();

    let members = search_symbols_scoped(&conn, "App.", None, None, false, 20).unwrap();

    let names: Vec<_> = members.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["run"]);
}

#[test]
fn the_kind_filter_accepts_the_attribute_kind_that_lookup_shows() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/app.py",
        "class App:\n    def __init__(self):\n        self.cli = 1\n\n    def run(self):\n        pass\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();

    let members = search_symbols_scoped(&conn, "App.", Some("attribute"), None, false, 20).unwrap();
    let exact =
        search_symbols_scoped(&conn, "App.cli", Some("attribute"), None, false, 20).unwrap();

    let names: Vec<_> = members.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["cli"]);
    assert_eq!(exact.len(), 1);
}

fn bare_callers(files: &[(&str, &str)], target: &str) -> Vec<String> {
    let (_repo, db_path) = scanned_repo(files);
    let conn = open_read_only(&db_path).unwrap();
    let refs = find_references_scoped(&conn, target, "callers", 20, false, None).unwrap();
    caller_names(&refs)
}

#[test]
fn a_bare_java_call_reaches_a_method_the_caller_inherits_from_a_base_in_another_file() {
    let callers = bare_callers(
        &[
            ("src/Base.java", "class Base {\n    void helper() {}\n}\n"),
            (
                "src/Child.java",
                "class Child extends Base {\n    void run() {\n        helper();\n    }\n}\n",
            ),
        ],
        "helper",
    );

    assert_eq!(callers, vec!["run"]);
}

#[test]
fn a_bare_swift_call_reaches_a_method_the_test_class_inherits() {
    let callers = bare_callers(
        &[(
            "Tests/CombineTests.swift",
            "class CombineTestCase {\n    func store(_ body: () -> Void) {}\n}\n\nfinal class DataTests: CombineTestCase {\n    func testPublish() {\n        store {\n        }\n    }\n}\n",
        )],
        "store",
    );

    assert_eq!(callers, vec!["testPublish"]);
}

#[test]
fn a_bare_csharp_call_reaches_a_method_in_another_part_of_a_partial_class() {
    let callers = bare_callers(
        &[(
            "src/LinqBridge.cs",
            "static partial class Enumerable\n{\n    private static void CheckNotNull(object source, string name) {}\n}\n\nstatic partial class Enumerable\n{\n    public static int Sum(object source)\n    {\n        CheckNotNull(source, \"source\");\n        return 0;\n    }\n}\n",
        )],
        "CheckNotNull",
    );

    assert_eq!(callers, vec!["Sum"]);
}

#[test]
fn a_bare_cpp_call_in_a_struct_method_reaches_a_function_of_the_enclosing_namespace() {
    let callers = bare_callers(
        &[(
            "src/doctest.h",
            "namespace doctest {\nconst char* assertString(int at) { return \"\"; }\nstruct JUnitReporter {\n    void log_assert() {\n        assertString(1);\n    }\n};\n}\n",
        )],
        "assertString",
    );

    assert_eq!(callers, vec!["log_assert"]);
}

#[test]
fn a_csharp_constructor_call_reaches_a_sibling_nested_class() {
    let callers = bare_callers(
        &[(
            "src/Tests.cs",
            "public class Tests\n{\n    public class RootSomethingElse {}\n\n    public class Something\n    {\n        public Something()\n        {\n            var other = new RootSomethingElse();\n        }\n    }\n}\n",
        )],
        "RootSomethingElse",
    );

    assert_eq!(callers, vec!["Something"]);
}

#[test]
fn a_bare_python_call_never_reaches_a_method_of_a_base_class() {
    let callers = bare_callers(
        &[(
            "src/app.py",
            "class Base:\n    def helper(self):\n        pass\n\n\nclass Child(Base):\n    def run(self):\n        helper()\n",
        )],
        "helper",
    );

    assert!(callers.is_empty(), "got {callers:?}");
}

#[test]
fn a_csharp_constructor_call_reaches_a_class_of_the_enclosing_namespace() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/ConverterTests.cs",
        "namespace Tests.Converters\n{\n    public class ConverterTests\n    {\n        public void ConverterDictionary()\n        {\n            var items = new ConverterDictionary<object>();\n        }\n    }\n\n    public class ConverterDictionary<T>\n    {\n    }\n}\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();
    let class_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'ConverterDictionary' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs =
        find_references_for_symbol(&conn, "ConverterDictionary", "callers", 20, &class_id).unwrap();

    assert!(
        refs.iter().any(|r| r.kind == "instantiates"),
        "got {refs:?}"
    );
}

#[test]
fn a_bare_call_in_a_nested_csharp_class_prefers_the_nearest_enclosing_definition() {
    let callers = bare_callers(
        &[(
            "src/Outer.cs",
            "public class Outer\n{\n    static void Log() {}\n\n    public class Middle\n    {\n        static void Log() {}\n\n        public class Inner\n        {\n            void Run()\n            {\n                Log();\n            }\n        }\n    }\n}\n",
        )],
        "Outer.Log",
    );

    assert!(callers.is_empty(), "got {callers:?}");
}

#[test]
fn a_bare_python_call_in_a_nested_function_never_reaches_a_method_of_the_enclosing_class() {
    let callers = bare_callers(
        &[(
            "src/app.py",
            "class App:\n    def helper(self):\n        pass\n\n    def outer(self):\n        def inner():\n            helper()\n        inner()\n",
        )],
        "helper",
    );

    assert!(callers.is_empty(), "got {callers:?}");
}

#[test]
fn a_this_call_in_an_object_literal_method_reaches_a_sibling_method() {
    let callers = bare_callers(
        &[(
            "src/input.js",
            "var Input = {\n  setInputValue: function (value) {\n    return value;\n  },\n  resetInputValue: function () {\n    this.setInputValue(1);\n  }\n};\n",
        )],
        "setInputValue",
    );

    assert_eq!(callers, vec!["resetInputValue"]);
}

#[test]
fn a_super_call_never_reaches_a_method_of_the_callers_own_class() {
    let callers = bare_callers(
        &[(
            "src/child.js",
            "class Base {\n  run() {}\n}\n\nclass Child extends Base {\n  run() {\n    super.run();\n  }\n}\n",
        )],
        "Child.run",
    );

    assert!(callers.is_empty(), "got {callers:?}");
}

#[test]
fn a_bare_ruby_call_follows_only_the_closest_base_class_of_that_name() {
    let (_repo, db_path) = scanned_repo(&[
        (
            "lib/app/base.rb",
            "module App\n  class Base\n    def options\n    end\n  end\nend\n",
        ),
        (
            "lib/guard/base.rb",
            "module Guard\n  class Base\n    def options\n    end\n  end\nend\n",
        ),
        (
            "lib/guard/token.rb",
            "module Guard\n  class Token < Base\n    def accepts\n      options\n    end\n  end\nend\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let options_in = |path: &str| -> Vec<String> {
        let id: String = conn
            .query_row(
                "SELECT symbol_id FROM symbols WHERE name = 'options' AND path = ?1",
                [path],
                |row| row.get(0),
            )
            .unwrap();
        caller_names(&find_references_for_symbol(&conn, "options", "callers", 20, &id).unwrap())
    };

    assert_eq!(options_in("lib/guard/base.rb"), vec!["accepts"]);
    assert!(options_in("lib/app/base.rb").is_empty());
}

#[test]
fn an_import_in_a_closer_scope_hides_a_definition_of_an_enclosing_scope() {
    let callers = bare_callers(
        &[(
            "src/scopes.cpp",
            "namespace outer {\nvoid helper() {}\nnamespace middle {\nusing tools::helper;\nstruct Inner {\n    void run() {\n        helper();\n    }\n};\n}\n}\n",
        )],
        "helper",
    );

    assert!(callers.is_empty(), "got {callers:?}");
}

#[test]
fn a_csharp_constructor_call_reaches_a_class_of_the_enclosing_file_scoped_namespace() {
    let (_repo, db_path) = scanned_repo(&[(
        "src/ConverterTests.cs",
        "namespace Tests.Converters;\n\npublic class ConverterTests\n{\n    public void ConverterDictionary()\n    {\n        var items = new ConverterDictionary<object>();\n    }\n}\n\npublic class ConverterDictionary<T>\n{\n}\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();
    let class_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'ConverterDictionary' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs =
        find_references_for_symbol(&conn, "ConverterDictionary", "callers", 20, &class_id).unwrap();

    assert!(
        refs.iter().any(|r| r.kind == "instantiates"),
        "got {refs:?}"
    );
}

#[test]
fn module_level_lambda_callers_allow_a_null_parent_and_keep_raw_names() {
    let (_repo, db) = scanned_repo(&[
        (
            "flask.py",
            "class Flask:\n    def __init__(self, name):\n        pass\n",
        ),
        (
            "app.py",
            "from flask import Flask\nmodule_app = lambda: Flask(\"module\")\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let flask_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'Flask' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let refs_result = find_references_for_symbol(&conn, "Flask", "callers", 20, &flask_id);
    assert!(
        refs_result.is_ok(),
        "module-level lambda failed: {refs_result:?}"
    );
    let refs = refs_result.unwrap();
    let lambda = refs
        .iter()
        .find(|site| site.from_symbol_name.starts_with("lambda_"))
        .expect("Flask should have a module-level lambda caller");
    let raw_name = lambda.from_symbol_name.as_str();
    assert!(lambda.enclosing_symbol_name.is_none());
    let text = format_references("Flask", &refs, "callers", 20);
    assert!(text.contains(&format!("`{raw_name}`")), "{text}");
    let json = serde_json::to_value(&refs).unwrap();
    assert!(json.as_array().unwrap().iter().any(|row| {
        row["from_symbol_name"].as_str() == Some(raw_name)
            && row.get("enclosing_symbol_name").is_none()
    }));

    let blast = compute_blast_radius(&conn, &["Flask"], &[], 2, 20).unwrap();
    let impacted = blast
        .impacted_symbols
        .iter()
        .find(|symbol| symbol.name == raw_name)
        .expect("blast radius should include the module-level lambda");
    assert!(impacted.enclosing_symbol_name.is_none());
    assert!(
        format_blast_radius(&blast).contains(&format!("`{raw_name}`")),
        "{}",
        format_blast_radius(&blast)
    );
}

#[test]
fn a_real_function_named_lambda_123_keeps_its_name_in_references_and_blast_radius() {
    let (_repo, db) = scanned_repo(&[
        (
            "flask.py",
            "class Flask:\n    def __init__(self, name):\n        pass\n",
        ),
        (
            "app.py",
            "from flask import Flask\ndef outer():\n    def lambda_123():\n        return Flask(\"real function\")\n    return lambda_123()\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let flask_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'Flask' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let refs = find_references_for_symbol(&conn, "Flask", "callers", 20, &flask_id).unwrap();
    let caller = refs
        .iter()
        .find(|site| site.from_symbol_name == "lambda_123")
        .expect("Flask should have a lambda_123 function caller");
    assert!(caller.enclosing_symbol_name.is_none());
    let references_text = format_references("Flask", &refs, "callers", 20);
    assert!(
        references_text.contains("`lambda_123`"),
        "{references_text}"
    );
    assert!(
        !references_text.contains("<lambda> in outer"),
        "{references_text}"
    );
    let references_json = serde_json::to_value(&refs).unwrap();
    assert!(references_json.as_array().unwrap().iter().any(|row| {
        row["from_symbol_name"] == "lambda_123" && row.get("enclosing_symbol_name").is_none()
    }));

    let blast = compute_blast_radius(&conn, &["Flask"], &[], 2, 20).unwrap();
    let impacted = blast
        .impacted_symbols
        .iter()
        .find(|symbol| symbol.name == "lambda_123")
        .expect("blast radius should include the lambda_123 function");
    assert!(impacted.enclosing_symbol_name.is_none());
    let blast_text = format_blast_radius(&blast);
    assert!(blast_text.contains("`lambda_123`"), "{blast_text}");
    assert!(!blast_text.contains("<lambda> in outer"), "{blast_text}");
    let blast_json = serde_json::to_value(&blast).unwrap();
    assert!(
        blast_json["impacted_symbols"]
            .as_array()
            .unwrap()
            .iter()
            .any(|row| {
                row["name"] == "lambda_123" && row.get("enclosing_symbol_name").is_none()
            })
    );
}

#[test]
fn lambda_callers_render_the_enclosing_symbol_without_changing_json_names() {
    let (_repo, db) = scanned_repo(&[
        (
            "flask.py",
            "class Flask:\n    def __init__(self, name):\n        pass\n",
        ),
        (
            "tests/test_cli.py",
            "class ScriptInfo:\n    def __init__(self, create_app):\n        self.create_app = create_app\n\ndef test_scriptinfo():\n    ScriptInfo(create_app=lambda: Flask(\"testapp\"))\n",
        ),
    ]);
    let conn = open_read_only(&db).unwrap();
    let flask_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'Flask' AND kind = 'class'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let refs = find_references_for_symbol(&conn, "Flask", "callers", 20, &flask_id).unwrap();
    let lambda = refs
        .iter()
        .find(|site| site.from_symbol_name.starts_with("lambda_"))
        .expect("Flask should have a lambda caller");

    let references_text = format_references("Flask", &refs, "callers", 20);
    assert!(
        references_text.contains("`<lambda> in test_scriptinfo`"),
        "{references_text}"
    );
    let references_json = serde_json::to_value(&refs).unwrap();
    let raw_lambda_name = lambda.from_symbol_name.as_str();
    let lambda_json = references_json
        .as_array()
        .unwrap()
        .iter()
        .find(|row| {
            row.get("from_symbol_name")
                .and_then(serde_json::Value::as_str)
                == Some(raw_lambda_name)
        })
        .expect("JSON should retain the raw lambda name");
    assert!(lambda_json.get("enclosing_symbol_name").is_none());

    let blast = compute_blast_radius(&conn, &["Flask"], &[], 2, 20).unwrap();
    let blast_text = format_blast_radius(&blast);
    assert!(
        blast_text.contains("`<lambda> in test_scriptinfo`"),
        "{blast_text}"
    );
    assert!(
        blast
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name.starts_with("lambda_")),
        "{:?}",
        blast.impacted_symbols
    );
    let blast_json = serde_json::to_value(&blast).unwrap();
    let lambda_json = blast_json["impacted_symbols"]
        .as_array()
        .unwrap()
        .iter()
        .find(|symbol| {
            symbol.get("name").and_then(serde_json::Value::as_str) == Some(raw_lambda_name)
        })
        .expect("JSON should retain the raw lambda name");
    assert!(lambda_json.get("enclosing_symbol_name").is_none());
}
