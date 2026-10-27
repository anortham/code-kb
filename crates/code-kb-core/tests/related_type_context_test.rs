use code_kb_core::{
    db::{link_impl_owners_path, open_read_only},
    ops::{SymbolSelector, get_context_slice_selected_op},
    queries::get_symbol_by_name,
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use rusqlite::Connection;
use std::{
    fs,
    path::{Path, PathBuf},
};

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace, PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in files {
        let full_path = root.join(path);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(full_path, content).unwrap();
    }
    let workspace = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&workspace, &db_path, true).expect("scan failed");
    (temp_dir, workspace, db_path)
}

fn related_types_for(
    conn: &Connection,
    workspace: &Workspace,
    db_path: &Path,
    file_path: &str,
    symbol_name: &str,
) -> Vec<String> {
    let target = get_symbol_by_name(conn, symbol_name, Some(file_path))
        .unwrap()
        .unwrap();
    get_context_slice_selected_op(
        workspace,
        db_path,
        conn,
        &SymbolSelector::Id(target.symbol_id),
        Some(file_path),
        false,
    )
    .unwrap()
    .related_types
}

fn assert_related_owner(types: &[String], name: &str, path: &str, line: usize) {
    let matches: Vec<_> = types
        .iter()
        .filter(|related_type| related_type.contains(name))
        .collect();
    assert_eq!(matches.len(), 1, "{name} owner mismatch: {types:?}");
    assert!(
        matches[0].ends_with(&format!("({path}:{line})")),
        "{name} resolved to the wrong owner: {types:?}"
    );
}

#[test]
fn typescript_contexts_choose_the_imported_type_over_foreign_same_named_types() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/review-2026-10-26/fixture")
        .canonicalize()
        .unwrap();
    let db_dir = safe_tempdir();
    let db_path = db_dir.path().join("artifact.db");
    let workspace = Workspace::new(fixture_root);
    scan_workspace(&workspace, &db_path, true).expect("scan failed");
    let conn = open_read_only(&db_path).unwrap();

    for (file_path, symbol_name) in [
        ("typescript/controllers/invitation.ts", "submitInvitation"),
        ("typescript/routes.ts", "postInvitation"),
        ("typescript/app.ts", "post"),
    ] {
        let related = related_types_for(&conn, &workspace, &db_path, file_path, symbol_name);
        let invitation_services = related
            .iter()
            .filter(|related_type| related_type.contains("InvitationService"))
            .collect::<Vec<_>>();
        assert_eq!(
            invitation_services,
            ["class InvitationService (typescript/service.ts:3)"],
            "{symbol_name} selected the wrong InvitationService: {related:?}"
        );
    }
}

#[test]
fn an_exact_relative_import_selects_its_type_over_a_shorter_same_language_decoy() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "typescript/services/invitations/core/service.ts",
            "export class InvitationService { send(): boolean { return true; } }\n",
        ),
        (
            "typescript/api/decoy.ts",
            "export class InvitationService { send(): boolean { return false; } }\n",
        ),
        (
            "typescript/api/controller.ts",
            "import { InvitationService } from '../services/invitations/core/service';\n\
             export function submit(service: InvitationService): boolean { return service.send(); }\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/api/controller.ts",
        "submit",
    );

    assert_eq!(
        related,
        ["class InvitationService (typescript/services/invitations/core/service.ts:1)"]
    );
}

#[test]
fn an_exact_typescript_import_can_bind_to_a_javascript_type() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "javascript/service.js",
            "export class BridgeService { send() { return true; } }\n",
        ),
        (
            "typescript/decoy.ts",
            "export class BridgeService { send(): boolean { return false; } }\n",
        ),
        (
            "typescript/controller.ts",
            "import { BridgeService } from '../javascript/service.js';\n\
             export function submit(service: BridgeService): boolean { return service.send(); }\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/controller.ts",
        "submit",
    );

    assert_eq!(related, ["class BridgeService (javascript/service.js:1)"]);
}

#[test]
fn aliased_imports_keep_distinct_same_named_type_owners() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "typescript/first/service.ts",
            "export class Service { first(): boolean { return true; } }\n",
        ),
        (
            "typescript/second/service.ts",
            "export class Service { second(): boolean { return true; } }\n",
        ),
        (
            "typescript/controller.ts",
            "import { Service as FirstService } from './first/service';\n\
             import { Service as SecondService } from './second/service';\n\
             export function submit(first: FirstService, second: SecondService): boolean { return true; }\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let mut related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/controller.ts",
        "submit",
    );
    related.sort();

    assert_eq!(
        related,
        [
            "class Service (typescript/first/service.ts:1)",
            "class Service (typescript/second/service.ts:1)"
        ]
    );
}

#[test]
fn an_unbound_same_language_type_name_does_not_choose_a_path_length_winner() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "typescript/controller.ts",
            "export function submit(service: InvitationService): boolean { return true; }\n",
        ),
        ("typescript/a.ts", "export class InvitationService {}\n"),
        (
            "typescript/deep/b.ts",
            "export class InvitationService {}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/controller.ts",
        "submit",
    );

    assert!(
        !related
            .iter()
            .any(|related_type| related_type.contains("InvitationService")),
        "ambiguous unbound type unexpectedly chose a candidate: {related:?}"
    );
}

#[test]
fn an_unindexed_external_import_does_not_bind_to_a_project_name_decoy() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "typescript/controller.ts",
            "import { InvitationService } from 'external-sdk';\n\
             export function submit(service: InvitationService): boolean { return true; }\n",
        ),
        ("typescript/decoy.ts", "export class InvitationService {}\n"),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/controller.ts",
        "submit",
    );

    assert!(
        !related
            .iter()
            .any(|related_type| related_type.contains("InvitationService")),
        "an external import selected a project name decoy: {related:?}"
    );
}

#[test]
fn indexed_package_and_namespace_facts_bind_cross_file_types() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "go/services/handler.go",
            "package services\n\ntype Handler struct { Service NotificationService }\n",
        ),
        (
            "go/services/service.go",
            "package services\n\ntype NotificationService struct {}\n",
        ),
        (
            "go/other/service.go",
            "package services\n\ntype NotificationService struct {}\n",
        ),
        (
            "java/review/Handler.java",
            "package review;\n\npublic class Handler { private NotificationService service; }\n",
        ),
        (
            "java/review/NotificationService.java",
            "package review;\n\npublic class NotificationService {}\n",
        ),
        (
            "java/other/NotificationService.java",
            "package other;\n\npublic class NotificationService {}\n",
        ),
        (
            "csharp/controllers/Handler.cs",
            "namespace Review;\n\npublic class Handler { public NotificationService Service { get; set; } }\n",
        ),
        (
            "csharp/domain/NotificationService.cs",
            "namespace Review;\n\npublic class NotificationService {}\n",
        ),
        (
            "csharp/other/NotificationService.cs",
            "namespace Other;\n\npublic class NotificationService {}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let go_related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "go/services/handler.go",
        "Handler",
    );
    let java_related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "java/review/Handler.java",
        "Handler",
    );
    let csharp_related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "csharp/controllers/Handler.cs",
        "Handler",
    );

    assert_related_owner(
        &go_related,
        "NotificationService",
        "go/services/service.go",
        3,
    );
    assert_related_owner(
        &java_related,
        "NotificationService",
        "java/review/NotificationService.java",
        3,
    );
    assert_related_owner(
        &csharp_related,
        "NotificationService",
        "csharp/domain/NotificationService.cs",
        3,
    );
}

#[test]
fn a_qualified_go_import_binds_by_the_indexed_module_path() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n",
        ),
        (
            "other/invitation.go",
            "package service\n\ntype InvitationService struct{}\n",
        ),
        (
            "handler/handler.go",
            "package handler\n\nimport \"fx/service\"\n\
             type Handler struct {\n\tservice *service.InvitationService\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(&conn, &workspace, &db_path, "handler/handler.go", "Handler");

    assert_related_owner(&related, "InvitationService", "service/invitation.go", 3);
}

#[test]
fn a_nested_go_module_blocks_an_outer_module_import_match() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        ("sub/go.mod", "module inner\n\ngo 1.21\n"),
        (
            "sub/service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n",
        ),
        (
            "handler/handler.go",
            "package handler\n\nimport \"fx/sub/service\"\n\
             type Handler struct {\n\tservice *service.InvitationService\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(&conn, &workspace, &db_path, "handler/handler.go", "Handler");

    assert!(
        !related
            .iter()
            .any(|related_type| related_type.contains("InvitationService")),
        "the outer go.mod bound a type inside a nested module: {related:?}"
    );
}

#[test]
fn cross_file_rust_impl_parent_binds_related_type_facts() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        ("src/lib.rs", "pub struct Handler;\n"),
        (
            "src/handler.rs",
            "use crate::Handler;\n\
             impl Handler {\n\
                 pub fn handle(other: Handler) {}\n\
             }\n",
        ),
    ]);
    link_impl_owners_path(&db_path).unwrap();
    let conn = open_read_only(&db_path).unwrap();
    let method = get_symbol_by_name(&conn, "handle", Some("src/handler.rs"))
        .unwrap()
        .unwrap();
    let owner = get_symbol_by_name(&conn, "Handler", Some("src/lib.rs"))
        .unwrap()
        .unwrap();
    assert_eq!(
        method.parent_symbol_id.as_deref(),
        Some(owner.symbol_id.as_str())
    );
    let related = related_types_for(&conn, &workspace, &db_path, "src/handler.rs", "handle");

    assert_related_owner(&related, "Handler", "src/lib.rs", 1);
}

#[test]
fn an_external_import_does_not_bind_to_a_same_named_project_module() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "typescript/controller.ts",
            "import { Service } from 'external-sdk';\n\
             export function submit(service: Service): boolean { return true; }\n",
        ),
        ("typescript/Service.ts", "export class Service {}\n"),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "typescript/controller.ts",
        "submit",
    );

    assert!(
        !related
            .iter()
            .any(|related_type| related_type.contains("Service")),
        "an external import selected a same-named project module: {related:?}"
    );
}

#[test]
fn java_fully_qualified_imports_use_indexed_namespaces_and_block_external_decoys() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "java/current/Handler.java",
            "package current;\nimport project.Service;\n\
             public class Handler { private Service service; }\n",
        ),
        (
            "java/project/Service.java",
            "package project;\n\npublic class Service {}\n",
        ),
        (
            "java/external/Handler.java",
            "package external;\nimport missing.Service;\n\
             public class Handler { private Service service; }\n",
        ),
        (
            "java/external/Service.java",
            "package external;\n\npublic class Service {}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let imported = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "java/current/Handler.java",
        "Handler",
    );
    let external = related_types_for(
        &conn,
        &workspace,
        &db_path,
        "java/external/Handler.java",
        "Handler",
    );

    assert_related_owner(&imported, "Service", "java/project/Service.java", 3);
    assert!(
        !external
            .iter()
            .any(|related_type| related_type.contains("Service")),
        "an external Java import selected its current-package decoy: {external:?}"
    );
}
