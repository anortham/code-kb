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

const CROSS_LANGUAGE_ANSWER_QUALITY: &[(&str, &str)] = &[
    (
        "services/auth_service.py",
        "def verify_credentials(user, password):\n    return user == password\n\ndef unrelated_audit():\n    return True\n",
    ),
    (
        "services/login_service.py",
        "from .auth_service import verify_credentials\n\ndef do_login(u, p):\n    return verify_credentials(u, p)\n",
    ),
    (
        "tests/test_auth.py",
        "from services.auth_service import verify_credentials\n\ndef test_verify_credentials():\n    assert verify_credentials('admin', 'admin')\n",
    ),
    (
        "tests/test_unrelated.py",
        "from services.auth_service import unrelated_audit\n\ndef test_unrelated_audit():\n    assert unrelated_audit()\n",
    ),
    (
        "src/user_store.rs",
        "pub struct UserStore;\nimpl UserStore {\n    pub fn save(&self) -> bool { true }\n}\npub struct OrderStore;\nimpl OrderStore {\n    pub fn save(&self) -> bool { true }\n}\npub fn register_user(store: &UserStore) -> bool {\n    UserStore::save(store)\n}\npub fn place_order(store: &OrderStore) -> bool {\n    OrderStore::save(store)\n}\npub fn helper_no_calls() -> usize {\n    42\n}\n",
    ),
    (
        "ts/auth.ts",
        "export function authenticateUser(token: string): boolean {\n    return token.length > 0;\n}\n",
    ),
    (
        "ts/client.ts",
        "import { authenticateUser } from './auth';\nexport function handleLogin(token: string): boolean {\n    return authenticateUser(token);\n}\nexport function handlePing(): string {\n    return 'pong';\n}\n",
    ),
];

#[test]
fn test_cross_language_callers_and_absent_matches() {
    let (_repo, db) = scanned_repo(CROSS_LANGUAGE_ANSWER_QUALITY);
    let conn = open_read_only(&db).unwrap();

    // 1. Rust method resolution with distinct receiver types
    let user_save =
        find_references_scoped(&conn, "UserStore::save", "callers", 20, false, None).unwrap();
    let user_callers = caller_names(&user_save);
    assert!(
        user_callers.contains(&"register_user".to_string()),
        "expected register_user in callers: {user_callers:?}"
    );
    assert!(
        !user_callers.contains(&"place_order".to_string()),
        "place_order must NOT be in UserStore::save callers (absent match)"
    );
    assert!(
        !user_callers.contains(&"helper_no_calls".to_string()),
        "helper_no_calls must NOT be in UserStore::save callers (absent match)"
    );

    let order_save =
        find_references_scoped(&conn, "OrderStore::save", "callers", 20, false, None).unwrap();
    let order_callers = caller_names(&order_save);
    assert!(
        order_callers.contains(&"place_order".to_string()),
        "expected place_order in callers: {order_callers:?}"
    );
    assert!(
        !order_callers.contains(&"register_user".to_string()),
        "register_user must NOT be in OrderStore::save callers (absent match)"
    );
    assert!(
        !order_callers.contains(&"helper_no_calls".to_string()),
        "helper_no_calls must NOT be in OrderStore::save callers (absent match)"
    );

    // 2. TypeScript function callers and negative controls
    let ts_refs = find_references_scoped(
        &conn,
        "authenticateUser",
        "callers",
        20,
        false,
        Some("ts/auth.ts"),
    )
    .unwrap();
    let ts_callers = caller_names(&ts_refs);
    assert!(
        ts_callers.contains(&"handleLogin".to_string()),
        "expected handleLogin in callers: {ts_callers:?}"
    );
    assert!(
        !ts_callers.contains(&"handlePing".to_string()),
        "handlePing must NOT be in authenticateUser callers (absent match)"
    );

    // 3. Python function callers and negative controls
    let py_refs = find_references_scoped(
        &conn,
        "verify_credentials",
        "callers",
        20,
        false,
        Some("services/auth_service.py"),
    )
    .unwrap();
    let py_callers = caller_names(&py_refs);
    assert!(
        py_callers.contains(&"do_login".to_string()),
        "expected do_login in callers: {py_callers:?}"
    );
    assert!(
        !py_callers.contains(&"unrelated_audit".to_string()),
        "unrelated_audit must NOT be in verify_credentials callers (absent match)"
    );
}

#[test]
fn test_blast_radius_predicted_tests_inclusion_and_absent_exclusion() {
    let (_repo, db) = scanned_repo(CROSS_LANGUAGE_ANSWER_QUALITY);
    let conn = open_read_only(&db).unwrap();

    let radius = compute_blast_radius_scoped(
        &conn,
        &["verify_credentials"],
        Some("services/auth_service.py"),
        &[],
        3,
        50,
    )
    .unwrap();

    let impacted_names: Vec<&str> = radius
        .impacted_symbols
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        impacted_names.contains(&"do_login"),
        "do_login should be impacted by verify_credentials: {impacted_names:?}"
    );
    assert!(
        !impacted_names.contains(&"unrelated_audit"),
        "unrelated_audit must NOT be impacted (absent match)"
    );

    let test_names: Vec<&str> = radius
        .likely_tests
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(
        test_names.contains(&"test_verify_credentials"),
        "expected test_verify_credentials in likely_tests: {test_names:?}"
    );
    assert!(
        !test_names.contains(&"test_unrelated_audit"),
        "test_unrelated_audit must NOT be in likely_tests for verify_credentials (absent match)"
    );
}

const ALIASED_QML_IMPORT: &[(&str, &str)] = &[
    ("Ui/Page.qml", "import QtQuick\n\nItem {\n    id: page\n}\n"),
    (
        "App/Main.qml",
        "import QtQuick\nimport \"../Ui\" as Ui\n\nItem {\n    Ui.Page {\n        id: body\n    }\n}\n",
    ),
];

const SAME_NAMED_METHODS_BEHIND_A_USE: &[(&str, &str)] = &[
    (
        "src/store.rs",
        "pub struct Store;\n\nimpl Store {\n    pub fn open() -> u8 {\n        1\n    }\n}\n",
    ),
    (
        "src/cache.rs",
        "pub struct Cache;\n\nimpl Cache {\n    pub fn open() -> u8 {\n        2\n    }\n}\n",
    ),
    (
        "src/boot.rs",
        "use crate::store::Store;\n\npub fn boot() -> u8 {\n    Store::open()\n}\n",
    ),
];

#[test]
fn an_import_alias_receiver_resolves_a_qualified_instantiation() {
    let (_repo, db) = scanned_repo(ALIASED_QML_IMPORT);
    let conn = open_read_only(&db).unwrap();

    let refs =
        find_references_scoped(&conn, "Page", "callers", 20, false, Some("Ui/Page.qml")).unwrap();

    let sites: Vec<(&str, &str, Option<usize>)> = refs
        .iter()
        .map(|r| (r.path.as_str(), r.kind.as_str(), r.start_line))
        .collect();
    assert_eq!(
        sites,
        vec![("App/Main.qml", "instantiates", Some(5))],
        "{refs:?}"
    );
}

#[test]
fn a_namespaced_call_keeps_its_parent_when_the_caller_imports_that_name() {
    let (_repo, db) = scanned_repo(SAME_NAMED_METHODS_BEHIND_A_USE);
    let conn = open_read_only(&db).unwrap();

    let from_store =
        find_references_scoped(&conn, "open", "callers", 20, false, Some("src/store.rs")).unwrap();
    let from_cache =
        find_references_scoped(&conn, "open", "callers", 20, false, Some("src/cache.rs")).unwrap();

    assert_eq!(caller_names(&from_store), ["boot"], "{from_store:?}");
    assert!(from_cache.is_empty(), "{from_cache:?}");
}

const QML_BASE_TYPES: &[(&str, &str)] = &[
    (
        "Ui/BarWidget.qml",
        "import QtQuick\n\nItem {\n    id: root\n}\n",
    ),
    (
        "clock/BarWidget.qml",
        "import QtQuick\n\nBarWidget {\n    id: clockBar\n}\n",
    ),
    (
        "media/Plugin.qml",
        "import QtQuick\n\nBarWidget {\n    id: plugin\n}\n",
    ),
];

fn repo_with_base_type_edges() -> (tempfile::TempDir, std::path::PathBuf) {
    scanned_repo(QML_BASE_TYPES)
}

#[test]
fn a_base_type_edge_resolves_to_the_base_and_never_to_the_file_that_shares_its_name() {
    let (_repo, db) = repo_with_base_type_edges();
    let conn = open_read_only(&db).unwrap();

    let base = find_references_scoped(
        &conn,
        "BarWidget",
        "callers",
        20,
        false,
        Some("Ui/BarWidget.qml"),
    )
    .unwrap();
    let mut sites: Vec<(&str, &str)> = base
        .iter()
        .map(|r| (r.path.as_str(), r.kind.as_str()))
        .collect();
    sites.sort();
    assert_eq!(
        sites,
        vec![
            ("clock/BarWidget.qml", "extends"),
            ("media/Plugin.qml", "extends")
        ],
        "{base:?}"
    );

    let derived = find_references_scoped(
        &conn,
        "BarWidget",
        "callers",
        20,
        false,
        Some("clock/BarWidget.qml"),
    )
    .unwrap();
    assert!(
        !derived.iter().any(|r| r.path == "clock/BarWidget.qml"),
        "{derived:?}"
    );
}

#[test]
fn the_impact_walk_follows_base_type_edges() {
    let (_repo, db) = repo_with_base_type_edges();
    let conn = open_read_only(&db).unwrap();

    let radius =
        compute_blast_radius_scoped(&conn, &[], None, &["Ui/BarWidget.qml"], 3, 50).unwrap();

    let mut impacted: Vec<(&str, &str)> = radius
        .impacted_symbols
        .iter()
        .map(|s| (s.path.as_str(), s.name.as_str()))
        .collect();
    impacted.sort();
    assert_eq!(
        impacted,
        vec![
            ("clock/BarWidget.qml", "BarWidget"),
            ("media/Plugin.qml", "Plugin")
        ],
        "{radius:?}"
    );
}

const QT_AND_WORKSPACE_MODULE_ALIASES: &[(&str, &str)] = &[
    (
        "src/controls/Page.qml",
        "import QtQuick\n\nItem {\n    id: page\n}\n",
    ),
    (
        "examples/ImagePage.qml",
        "import QtQuick\nimport QtQuick.Controls as QQC2\n\nItem {\n    QQC2.Page {\n        id: body\n    }\n}\n",
    ),
    (
        "autotests/TestPage.qml",
        "import QtQuick\nimport org.kde.kirigami as Kirigami\n\nItem {\n    Kirigami.Page {\n        id: probe\n    }\n}\n",
    ),
];

#[test]
fn an_alias_of_a_qt_module_does_not_resolve_to_a_workspace_component() {
    let (_repo, db) = scanned_repo(QT_AND_WORKSPACE_MODULE_ALIASES);
    let conn = open_read_only(&db).unwrap();

    let refs = find_references_scoped(
        &conn,
        "Page",
        "callers",
        20,
        false,
        Some("src/controls/Page.qml"),
    )
    .unwrap();

    let sites: Vec<(&str, &str, Option<usize>)> = refs
        .iter()
        .map(|r| (r.path.as_str(), r.kind.as_str(), r.start_line))
        .collect();
    assert_eq!(
        sites,
        vec![("autotests/TestPage.qml", "instantiates", Some(5))],
        "{refs:?}"
    );
}
