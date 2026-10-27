use std::collections::BTreeMap;
use std::fs;

use code_kb_core::queries::{compute_blast_radius_for_changes, find_callee_signatures};
use code_kb_core::{
    Symbol, Workspace, compute_blast_radius_scoped, find_julie_extract_binary, find_related_tests,
    get_context_slice_op, get_symbol_by_name, open_read_only, safe_tempdir, scan_workspace,
};

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

fn assert_compatible_call_edge(conn: &mut rusqlite::Connection, caller: &Symbol, target: &Symbol) {
    let tx = conn.transaction().unwrap();
    let relationship_id = tx
        .query_row(
            "SELECT relationship_id
             FROM relationships WHERE kind = 'calls' LIMIT 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .expect("the scan records a call relationship that can represent an indexed call fact");
    tx.execute(
        "UPDATE relationships SET from_symbol_id = ?1, to_symbol_id = ?2
         WHERE relationship_id = ?3",
        rusqlite::params![caller.symbol_id, target.symbol_id, relationship_id],
    )
    .unwrap();

    let impact = compute_blast_radius_scoped(
        &tx,
        &[target.name.as_str()],
        Some(target.path.as_str()),
        &[],
        3,
        100,
    )
    .unwrap();
    assert!(
        impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == caller.path),
        "{} did not reach {} through a compatible {} to {} call fact: {:?}",
        target.path,
        caller.path,
        caller.language,
        target.language,
        impact.impacted_symbols
    );

    let callee_signatures =
        find_callee_signatures(&tx, &caller.name, &caller.symbol_id, 20, false).unwrap();
    assert!(
        callee_signatures
            .iter()
            .any(|signature| signature.contains(&target.path))
    );
    tx.rollback().unwrap();
}

const CROSS_LANGUAGE_FILES: &[(&str, &str)] = &[
    ("go.mod", "module example.com/review\n\ngo 1.23\n"),
    (
        "go/invitation.go",
        "package invitation\n\ntype InvitationService struct{}\n\nfunc (s *InvitationService) Send(email string) bool { return true }\n\nfunc DirectSend(service *InvitationService) bool { return service.Send(\"test\") }\n\ntype InvitationHandler struct { service *InvitationService }\n\nfunc (h *InvitationHandler) Handle(email string) bool { return h.service.Send(email) }\n",
    ),
    (
        "go/invitation_test.go",
        "package invitation\n\nfunc TestSend() {\n    service := &InvitationService{}\n    _ = service.Send(\"test\")\n    handler := InvitationHandler{service: service}\n    _ = handler.Handle(\"test\")\n}\n",
    ),
    (
        "dotnet/InvitationService.cs",
        "namespace Review;\n\npublic class InvitationService\n{\n    public bool Send(string email) => true;\n}\n",
    ),
    (
        "dotnet/InvitationHandler.cs",
        "namespace Review;\n\npublic class InvitationHandler\n{\n    private readonly InvitationService service = new();\n\n    public bool Handle(string email) => service.Send(email);\n}\n",
    ),
    (
        "dotnet/tests/InvitationTests.cs",
        "namespace Review;\n\npublic class InvitationTests\n{\n    public bool TestSend()\n    {\n        var service = new InvitationService();\n        _ = service.Send(\"test\");\n        var handler = new InvitationHandler();\n        return handler.Handle(\"test\");\n    }\n}\n",
    ),
    (
        "typescript/Target.ts",
        "export function send() { return true; }\nexport function jsInteropTarget() { return true; }\n",
    ),
    (
        "typescript/Callers.ts",
        "import { send } from './Target';\nexport function tsDirect() { return send(); }\nexport function tsDeep() { return tsDirect(); }\n",
    ),
    (
        "javascript/Callers.js",
        "import { send, jsInteropTarget } from '../typescript/Target';\nexport function jsDirect() { return send(); }\nexport function jsInterop() { return jsInteropTarget(); }\n",
    ),
    (
        "java/Target.java",
        "package review;\nclass JavaTarget {\n    boolean send() { return true; }\n    boolean jvmInteropTarget() { return true; }\n}\n",
    ),
    (
        "java/Callers.java",
        "package review;\nclass JavaCallers {\n    boolean javaDirect() { return JavaTarget.send(); }\n    boolean javaDeep() { return javaDirect(); }\n}\n",
    ),
    (
        "java/KotlinCallers.kt",
        "package review\nclass KotlinCallers {\n    fun interop(): Boolean {\n        return JavaTarget().jvmInteropTarget()\n    }\n}\n",
    ),
    (
        "rust/Cargo.toml",
        "[package]\nname = \"review\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    ("rust/src/lib.rs", "pub mod callers;\npub mod target;\n"),
    ("rust/src/target.rs", "pub fn send() -> bool { true }\n"),
    (
        "rust/src/callers.rs",
        "use crate::target::send;\npub fn rustDirect() -> bool { send() }\npub fn rustDeep() -> bool { rustDirect() }\n",
    ),
    (
        "dotnet/DotnetTarget.cs",
        "namespace Review;\npublic class DotnetTarget { public void Run() {} }\n",
    ),
    (
        "dotnet/VbCaller.vb",
        "Namespace Review\nPublic Class VbCaller\n    Public Sub Interop()\n        New DotnetTarget().Run()\n    End Sub\nEnd Class\nEnd Namespace\n",
    ),
    ("native/Target.c", "void cInteropTarget(void) {}\n"),
    (
        "native/Caller.cpp",
        "extern \"C\" void cInteropTarget(void);\nvoid cppInterop() { cInteropTarget(); }\n",
    ),
];

#[test]
fn call_language_guard_keeps_graph_and_context_edges_in_compatible_languages() {
    let (repo, db_path) = scanned_repo(CROSS_LANGUAGE_FILES);
    let root = repo.path().to_path_buf();
    let workspace = Workspace::new(root);
    let conn = open_read_only(&db_path).unwrap();

    let go_impact =
        compute_blast_radius_scoped(&conn, &["Send"], Some("go/invitation.go"), &[], 4, 100)
            .unwrap();
    let go_target = get_symbol_by_name(&conn, "Send", Some("go/invitation.go"))
        .unwrap()
        .unwrap();
    assert_eq!(go_target.language, "go");

    assert!(
        go_impact
            .impacted_symbols
            .iter()
            .any(|symbol| { symbol.name == "DirectSend" && symbol.path == "go/invitation.go" })
    );
    assert!(
        !go_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("dotnet/"))
    );
    assert!(
        !go_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("dotnet/"))
    );

    let csharp_impact = compute_blast_radius_scoped(
        &conn,
        &["Send"],
        Some("dotnet/InvitationService.cs"),
        &[],
        4,
        100,
    )
    .unwrap();
    assert!(
        csharp_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "dotnet/InvitationHandler.cs")
    );
    assert!(
        !csharp_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("go/"))
    );
    assert!(
        csharp_impact
            .likely_tests
            .iter()
            .any(|test| test.path == "dotnet/tests/InvitationTests.cs")
    );
    assert!(
        !csharp_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("go/"))
    );

    let go_context = get_context_slice_op(
        &workspace,
        &db_path,
        &conn,
        "DirectSend",
        Some("go/invitation.go"),
        false,
    )
    .unwrap();
    assert!(
        go_context
            .callee_signatures
            .iter()
            .any(|callee| callee.contains("go/invitation.go"))
    );
    assert!(
        !go_context
            .callee_signatures
            .iter()
            .any(|callee| callee.starts_with("InvitationService.Send (dotnet/"))
    );
    assert!(
        !go_context
            .related_types
            .iter()
            .any(|related_type| related_type.contains("dotnet/"))
    );

    let csharp_handler = get_context_slice_op(
        &workspace,
        &db_path,
        &conn,
        "Handle",
        Some("dotnet/InvitationHandler.cs"),
        false,
    )
    .unwrap();
    assert!(
        csharp_handler
            .callee_signatures
            .iter()
            .any(|callee| callee.contains("dotnet/InvitationService.cs"))
    );
    assert!(
        !csharp_handler
            .callee_signatures
            .iter()
            .any(|callee| callee.contains("(go/"))
    );
    assert!(
        !csharp_handler
            .related_types
            .iter()
            .any(|related_type| related_type.contains("go/"))
    );
    assert!(
        csharp_handler
            .related_tests
            .iter()
            .any(|test| test.path == "dotnet/tests/InvitationTests.cs")
    );
    assert!(
        !csharp_handler
            .related_tests
            .iter()
            .any(|test| test.path.starts_with("go/")),
        "{:?}",
        csharp_handler.related_tests
    );

    let csharp_handler_symbol =
        get_symbol_by_name(&conn, "Handle", Some("dotnet/InvitationHandler.cs"))
            .unwrap()
            .unwrap();
    let csharp_callees = code_kb_core::queries::find_references_for_symbol(
        &conn,
        &csharp_handler_symbol.name,
        "callees",
        20,
        &csharp_handler_symbol.symbol_id,
    )
    .unwrap();
    assert!(csharp_callees.iter().any(|site| {
        site.to_symbol_name == "Send"
            && site
                .target
                .as_deref()
                .is_some_and(|target| target.contains("(dotnet/InvitationService.cs:"))
    }));
    assert!(!csharp_callees.iter().any(|site| {
        site.to_symbol_name == "Send"
            && site
                .target
                .as_deref()
                .is_some_and(|target| target.contains("(go/"))
    }));

    let csharp_target = get_symbol_by_name(&conn, "Send", Some("dotnet/InvitationService.cs"))
        .unwrap()
        .unwrap();
    let go_related = find_related_tests(&conn, &go_target, 20).unwrap();
    assert!(
        go_related
            .iter()
            .any(|test| test.path == "go/invitation_test.go")
    );
    assert!(
        !go_related
            .iter()
            .any(|test| test.path.starts_with("dotnet/"))
    );
    let csharp_related = find_related_tests(&conn, &csharp_target, 20).unwrap();
    assert!(
        csharp_related
            .iter()
            .any(|test| test.path == "dotnet/tests/InvitationTests.cs")
    );
    assert!(
        !csharp_related
            .iter()
            .any(|test| test.path.starts_with("go/"))
    );

    for (path, expected, excluded) in [
        (
            "typescript/Target.ts",
            &[
                ("typescript/Callers.ts", "tsDirect", 1),
                ("typescript/Callers.ts", "tsDeep", 2),
                ("javascript/Callers.js", "jsDirect", 1),
            ][..],
            &["java/Callers.java", "rust/src/callers.rs"][..],
        ),
        (
            "java/Target.java",
            &[
                ("java/Callers.java", "javaDirect", 1),
                ("java/Callers.java", "javaDeep", 2),
            ][..],
            &["typescript/Callers.ts", "rust/src/callers.rs"][..],
        ),
        (
            "rust/src/target.rs",
            &[
                ("rust/src/callers.rs", "rustDirect", 1),
                ("rust/src/callers.rs", "rustDeep", 2),
            ][..],
            &["typescript/Callers.ts", "java/Callers.java"][..],
        ),
    ] {
        let impact =
            compute_blast_radius_scoped(&conn, &["send"], Some(path), &[], 4, 100).unwrap();
        for (expected_path, expected_name, expected_depth) in expected {
            assert!(
                impact.impacted_symbols.iter().any(|symbol| {
                    symbol.path == *expected_path
                        && symbol.name == *expected_name
                        && symbol.depth == *expected_depth
                }),
                "{path} did not reach {expected_name} at depth {expected_depth}: {:?}",
                impact.impacted_symbols
            );
        }
        for excluded_path in excluded {
            assert!(
                !impact
                    .impacted_symbols
                    .iter()
                    .any(|symbol| symbol.path == *excluded_path),
                "{path} reached incompatible {excluded_path}: {:?}",
                impact.impacted_symbols
            );
        }
    }

    let ts_interop = compute_blast_radius_scoped(
        &conn,
        &["jsInteropTarget"],
        Some("typescript/Target.ts"),
        &[],
        3,
        100,
    )
    .unwrap();
    assert!(
        ts_interop
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "javascript/Callers.js")
    );

    let kotlin_caller = get_symbol_by_name(&conn, "interop", Some("java/KotlinCallers.kt"))
        .unwrap()
        .unwrap();
    let java_target = get_symbol_by_name(&conn, "jvmInteropTarget", Some("java/Target.java"))
        .unwrap()
        .unwrap();
    let vb_caller = get_symbol_by_name(&conn, "Interop", Some("dotnet/VbCaller.vb"))
        .unwrap()
        .unwrap();
    let csharp_target = get_symbol_by_name(&conn, "Run", Some("dotnet/DotnetTarget.cs"))
        .unwrap()
        .unwrap();
    let cpp_caller = get_symbol_by_name(&conn, "cppInterop", Some("native/Caller.cpp"))
        .unwrap()
        .unwrap();
    let c_target = get_symbol_by_name(&conn, "cInteropTarget", Some("native/Target.c"))
        .unwrap()
        .unwrap();
    drop(conn);

    let mut write_conn = code_kb_core::db::open_read_write(&db_path).unwrap();
    assert_compatible_call_edge(&mut write_conn, &kotlin_caller, &java_target);
    assert_compatible_call_edge(&mut write_conn, &vb_caller, &csharp_target);
    assert_compatible_call_edge(&mut write_conn, &cpp_caller, &c_target);

    let mut go_changes = BTreeMap::new();
    go_changes.insert("go/invitation.go".to_string(), vec![(5, 5)]);
    let go_diff_impact =
        compute_blast_radius_for_changes(&write_conn, &go_changes, 4, 100).unwrap();
    assert!(
        go_diff_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == "DirectSend" && symbol.path == "go/invitation.go")
    );
    assert!(
        go_diff_impact
            .likely_tests
            .iter()
            .any(|test| test.path == "go/invitation_test.go")
    );
    assert!(
        !go_diff_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("dotnet/"))
    );
    assert!(
        !go_diff_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("dotnet/"))
    );

    let mut csharp_changes = BTreeMap::new();
    csharp_changes.insert("dotnet/InvitationService.cs".to_string(), vec![(5, 5)]);
    let csharp_diff_impact =
        compute_blast_radius_for_changes(&write_conn, &csharp_changes, 4, 100).unwrap();
    assert!(
        csharp_diff_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "dotnet/InvitationHandler.cs")
    );
    assert!(
        csharp_diff_impact
            .likely_tests
            .iter()
            .any(|test| test.path == "dotnet/tests/InvitationTests.cs")
    );
    assert!(
        !csharp_diff_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("go/"))
    );
    assert!(
        !csharp_diff_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("go/"))
    );

    let go_file_impact =
        compute_blast_radius_scoped(&write_conn, &[], None, &["go/invitation.go"], 4, 100).unwrap();
    assert!(
        go_file_impact
            .likely_tests
            .iter()
            .any(|test| test.path == "go/invitation_test.go"),
        "{go_file_impact:?}"
    );
    assert!(
        !go_file_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("dotnet/")),
        "{go_file_impact:?}"
    );
    assert!(
        !go_file_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("dotnet/"))
    );

    let csharp_file_impact = compute_blast_radius_scoped(
        &write_conn,
        &[],
        None,
        &["dotnet/InvitationService.cs"],
        4,
        100,
    )
    .unwrap();
    assert!(
        csharp_file_impact
            .likely_tests
            .iter()
            .any(|test| test.path == "dotnet/tests/InvitationTests.cs"),
        "{csharp_file_impact:?}"
    );
    assert!(
        !csharp_file_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("go/"))
    );
    assert!(
        !csharp_file_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("go/"))
    );

    write_conn
        .execute(
            "ALTER TABLE pending_relationships DROP COLUMN target_namespace_json",
            [],
        )
        .unwrap();
    let legacy_impact = compute_blast_radius_scoped(
        &write_conn,
        &["Send"],
        Some("go/invitation.go"),
        &[],
        4,
        100,
    )
    .unwrap();
    assert!(
        legacy_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == "DirectSend" && symbol.path == "go/invitation.go")
    );
    assert!(
        !legacy_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("dotnet/"))
    );
}

#[test]
fn pending_type_usages_and_instantiations_do_not_cross_language_graph_edges() {
    let (_repo, db_path) = scanned_repo(&[
        ("go.mod", "module example.com/type-usage\n\ngo 1.23\n"),
        (
            "go/invitation.go",
            "package invitation\n\ntype InvitationService struct{}\n\nfunc (s *InvitationService) Send(email string) bool { return true }\n",
        ),
        (
            "go/invitation_test.go",
            "package invitation\n\nfunc TestSend() {\n    service := &InvitationService{}\n    _ = service.Send(\"test\")\n}\n",
        ),
        (
            "dotnet/InvitationService.cs",
            "namespace Review;\npublic class InvitationService { public bool Send(string email) => true; }\n",
        ),
        (
            "dotnet/InvitationHandler.cs",
            "namespace Review;\npublic class InvitationHandler { private readonly InvitationService service = new(); public bool Handle(string email) => service.Send(email); }\n",
        ),
        (
            "dotnet/tests/InvitationTests.cs",
            "namespace Review;\npublic class InvitationTests { public bool TestSend() { var service = new InvitationService(); return service.Send(\"test\"); } }\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let go_owner = get_symbol_by_name(&conn, "InvitationService", Some("go/invitation.go"))
        .unwrap()
        .unwrap();

    let pending_types = conn
        .prepare(
            "SELECT DISTINCT p.kind, s_from.path, s_from.language
             FROM pending_relationships p
             JOIN symbols s_from ON s_from.symbol_id = p.from_symbol_id
             JOIN symbols s_target ON s_target.name = p.target_terminal_name
             WHERE s_target.symbol_id = ?1 AND s_from.path LIKE 'dotnet/%'",
        )
        .unwrap()
        .query_map([&go_owner.symbol_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(pending_types.iter().any(|(kind, path, language)| {
        kind == "uses" && path == "dotnet/InvitationHandler.cs" && language == "csharp"
    }));
    assert!(pending_types.iter().any(|(kind, path, language)| {
        kind == "instantiates" && path == "dotnet/tests/InvitationTests.cs" && language == "csharp"
    }));

    let csharp_test =
        get_symbol_by_name(&conn, "TestSend", Some("dotnet/tests/InvitationTests.cs"))
            .unwrap()
            .unwrap();
    let dependencies =
        find_callee_signatures(&conn, &csharp_test.name, &csharp_test.symbol_id, 20, false)
            .unwrap();
    assert!(
        dependencies
            .iter()
            .any(|dependency| dependency.contains("(dotnet/InvitationService.cs:"))
    );
    assert!(
        !dependencies
            .iter()
            .any(|dependency| dependency.contains("(go/invitation.go:")),
        "{dependencies:?}"
    );

    let (callee_sites, _) = code_kb_core::queries::find_callees_page_for_symbol(
        &conn,
        &csharp_test.name,
        &csharp_test.symbol_id,
        20,
        0,
        false,
    )
    .unwrap();
    assert!(
        callee_sites.iter().any(|site| {
            site.target
                .as_deref()
                .is_some_and(|target| target.contains("dotnet/InvitationService.cs:"))
        }),
        "{callee_sites:?}"
    );
    assert!(
        !callee_sites.iter().any(|site| {
            site.target
                .as_deref()
                .is_some_and(|target| target.contains("go/invitation.go:"))
        }),
        "{callee_sites:?}"
    );

    let callees = code_kb_core::queries::find_references_for_symbol(
        &conn,
        &csharp_test.name,
        "callees",
        20,
        &csharp_test.symbol_id,
    )
    .unwrap();
    assert!(
        !callees.iter().any(|site| {
            site.target
                .as_deref()
                .is_some_and(|target| target.contains("(go/invitation.go:"))
        }),
        "{callees:?}"
    );

    let file_impact =
        compute_blast_radius_scoped(&conn, &[], None, &["go/invitation.go"], 4, 100).unwrap();
    assert!(
        !file_impact
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path.starts_with("dotnet/")),
        "{file_impact:?}"
    );
    assert!(
        !file_impact
            .likely_tests
            .iter()
            .any(|test| test.path.starts_with("dotnet/")),
        "{file_impact:?}"
    );

    let go_tests = find_related_tests(&conn, &go_owner, 20).unwrap();
    assert!(
        go_tests
            .iter()
            .any(|test| test.path == "go/invitation_test.go")
    );
    assert!(
        !go_tests.iter().any(|test| test.path.starts_with("dotnet/")),
        "{go_tests:?}"
    );

    let csharp_owner = get_symbol_by_name(
        &conn,
        "InvitationService",
        Some("dotnet/InvitationService.cs"),
    )
    .unwrap()
    .unwrap();
    let csharp_tests = find_related_tests(&conn, &csharp_owner, 20).unwrap();
    assert!(
        !csharp_tests.iter().any(|test| test.path.starts_with("go/")),
        "{csharp_tests:?}"
    );
}

#[test]
fn reported_call_fallback_keeps_same_language_tests() {
    let (_repo, db_path) = scanned_repo(&[(
        "tests/test_fallback.py",
        "def test_0_target(depth=0):\n    if depth > 0:\n        return test_0_target(depth - 1)\n\ndef test_1_other():\n    test_0_target(1)\n",
    )]);
    let conn = open_read_only(&db_path).unwrap();
    let target = get_symbol_by_name(&conn, "test_0_target", None)
        .unwrap()
        .unwrap();
    let other_test = get_symbol_by_name(&conn, "test_1_other", None)
        .unwrap()
        .unwrap();
    let callers = code_kb_core::queries::find_references_for_symbol(
        &conn,
        &target.name,
        "callers",
        20,
        &target.symbol_id,
    )
    .unwrap();
    assert!(
        callers
            .iter()
            .any(|site| site.from_symbol_id == target.symbol_id && site.kind == "calls"),
        "{callers:?}"
    );
    assert!(
        callers
            .iter()
            .any(|site| site.from_symbol_id == other_test.symbol_id && site.kind == "calls"),
        "{callers:?}"
    );
    let tests = find_related_tests(&conn, &target, 1).unwrap();
    assert!(
        tests
            .iter()
            .any(|related_test| related_test.symbol_id == other_test.symbol_id),
        "{tests:?}"
    );
}
