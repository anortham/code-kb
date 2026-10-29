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

fn assert_no_related(types: &[String], name: &str) {
    assert!(
        !types.iter().any(|related_type| related_type.contains(name)),
        "{name} bound without import evidence: {types:?}"
    );
}

#[test]
fn python_relative_absolute_and_reexported_imports_bind_the_imported_class() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        ("pkg/__init__.py", "from .ctx import AppContext\n"),
        ("pkg/ctx.py", "class AppContext:\n    pass\n"),
        ("pkg/sub/__init__.py", ""),
        ("pkg/sub/ctx.py", "class AppContext:\n    pass\n"),
        (
            "pkg/app.py",
            "from .ctx import AppContext\n\n\ndef handle(ctx: AppContext):\n    return ctx\n",
        ),
        (
            "pkg/sub/view.py",
            "from ..ctx import AppContext\n\n\ndef view(ctx: AppContext):\n    return ctx\n",
        ),
        (
            "pkg/client.py",
            "from pkg import AppContext\n\n\ndef use(ctx: AppContext):\n    return ctx\n",
        ),
        (
            "pkg/absolute.py",
            "from pkg.sub.ctx import AppContext\n\n\ndef run(ctx: AppContext):\n    return ctx\n",
        ),
        (
            "pkg/external.py",
            "from werkzeug.local import LocalProxy\n\n\ndef wrap(proxy: LocalProxy):\n    return proxy\n",
        ),
        ("pkg/local.py", "class LocalProxy:\n    pass\n"),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related =
        |path: &str, name: &str| related_types_for(&conn, &workspace, &db_path, path, name);

    assert_related_owner(
        &related("pkg/app.py", "handle"),
        "AppContext",
        "pkg/ctx.py",
        1,
    );
    assert_related_owner(
        &related("pkg/sub/view.py", "view"),
        "AppContext",
        "pkg/ctx.py",
        1,
    );
    assert_related_owner(
        &related("pkg/client.py", "use"),
        "AppContext",
        "pkg/ctx.py",
        1,
    );
    assert_related_owner(
        &related("pkg/absolute.py", "run"),
        "AppContext",
        "pkg/sub/ctx.py",
        1,
    );
    assert_no_related(&related("pkg/external.py", "wrap"), "LocalProxy");
}

#[test]
fn rust_use_paths_bind_the_type_of_the_named_module() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "crates/my-core/Cargo.toml",
            "[package]\nname = \"my-core\"\n",
        ),
        (
            "crates/my-core/src/lib.rs",
            "pub mod db;\npub mod models;\npub mod other;\npub mod report;\npub use models::Symbol;\n",
        ),
        ("crates/my-core/src/models.rs", "pub struct Symbol {}\n"),
        ("crates/my-core/src/other.rs", "pub struct Symbol {}\n"),
        ("crates/my-core/src/db.rs", "pub struct Connection {}\n"),
        (
            "crates/my-core/src/report/mod.rs",
            "pub mod arity;\npub struct Accounting {}\n",
        ),
        (
            "crates/my-core/src/report/arity.rs",
            "use crate::models::Symbol;\nuse rusqlite::Connection;\nuse super::{Accounting};\n\n\
             pub fn count(symbol: &Symbol, accounting: &Accounting, conn: &Connection) -> usize { 0 }\n",
        ),
        (
            "crates/my-core/src/report/tree.rs",
            "use crate::{db::Connection, models::{Symbol}};\n\n\
             pub fn tree(symbol: Symbol, conn: Connection) {}\n",
        ),
        ("crates/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "crates/app/src/main.rs",
            "use my_core::Symbol;\n\nfn run(symbol: Symbol) {}\n\nfn main() {}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related =
        |path: &str, name: &str| related_types_for(&conn, &workspace, &db_path, path, name);

    let arity = related("crates/my-core/src/report/arity.rs", "count");
    assert_related_owner(&arity, "Symbol", "crates/my-core/src/models.rs", 1);
    assert_related_owner(&arity, "Accounting", "crates/my-core/src/report/mod.rs", 2);
    assert_no_related(&arity, "Connection");
    let tree = related("crates/my-core/src/report/tree.rs", "tree");
    assert_related_owner(&tree, "Symbol", "crates/my-core/src/models.rs", 1);
    assert_related_owner(&tree, "Connection", "crates/my-core/src/db.rs", 1);
    assert_related_owner(
        &related("crates/app/src/main.rs", "run"),
        "Symbol",
        "crates/my-core/src/models.rs",
        1,
    );
}

#[test]
fn csharp_and_razor_types_bind_through_enclosing_namespaces_and_usings() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "App/Rendering/Measurer.cs",
            "using App.Data;\n\nnamespace App.Rendering\n{\n    public class Measurer\n    {\n        \
             public bool Valid(ScaleBase scale, Row row) { return true; }\n    }\n}\n",
        ),
        (
            "App/ScaleBase.cs",
            "namespace App\n{\n    public abstract class ScaleBase {}\n}\n",
        ),
        (
            "App/Data/Row.cs",
            "namespace App.Data\n{\n    public class Row {}\n}\n",
        ),
        (
            "Other/Row.cs",
            "namespace Other\n{\n    public class Row {}\n}\n",
        ),
        (
            "Other/ScaleBase.cs",
            "namespace Other\n{\n    public abstract class ScaleBase {}\n}\n",
        ),
        (
            "Other/Rendering/Plain.cs",
            "namespace Plain\n{\n    public class Plain\n    {\n        \
             public bool Valid(ScaleBase scale) { return true; }\n    }\n}\n",
        ),
        (
            "App/Data/Reader.cs",
            "namespace App.Data\n{\n    public partial class Reader {}\n}\n",
        ),
        (
            "App/Data/Reader.Async.cs",
            "namespace App.Data\n{\n    public partial class Reader {}\n}\n",
        ),
        (
            "App/Rendering/Loader.cs",
            "using App.Data;\n\nnamespace App.Rendering\n{\n    public class Loader\n    {\n        \
             public void Load(Reader reader) { }\n    }\n}\n",
        ),
        ("Web/_Imports.razor", "@using App.Data\n"),
        (
            "Web/Pages/Grid.razor",
            "<h1>Grid</h1>\n@code {\n    void Load(Row row) { }\n}\n",
        ),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related =
        |path: &str, name: &str| related_types_for(&conn, &workspace, &db_path, path, name);

    let measurer = related("App/Rendering/Measurer.cs", "Valid");
    assert_related_owner(&measurer, "ScaleBase", "App/ScaleBase.cs", 3);
    assert_related_owner(&measurer, "Row", "App/Data/Row.cs", 3);
    assert_no_related(&related("Other/Rendering/Plain.cs", "Valid"), "ScaleBase");
    assert_related_owner(
        &related("App/Rendering/Loader.cs", "Load"),
        "Reader",
        "App/Data/Reader.cs",
        3,
    );
    assert_related_owner(
        &related("Web/Pages/Grid.razor", "Load"),
        "Row",
        "App/Data/Row.cs",
        3,
    );
}

#[test]
fn c_types_bind_through_includes_and_their_includes() {
    let (_repo, workspace, db_path) = scanned_repo(&[
        (
            "src/jv.h",
            "#ifdef __cplusplus\nextern \"C\" {\n#endif\ntypedef struct { int kind; } jv;\n\
             #ifdef __cplusplus\n}\n#endif\n",
        ),
        (
            "include/lib/detail/position.h",
            "typedef struct { int line; } position_t;\n",
        ),
        (
            "tests/abi/position.h",
            "typedef struct { int line; } position_t;\n",
        ),
        (
            "src/error.c",
            "#include <lib/detail/position.h>\n\nint where(position_t pos) { return pos.line; }\n",
        ),
        (
            "src/jq.h",
            "#include \"jv.h\"\n\njv jq_get_exit_code(void);\n",
        ),
        (
            "src/main.c",
            "#include <stdio.h>\n#include \"jq.h\"\n\njv run(jv value) { return value; }\n",
        ),
        ("other/jv.h", "typedef struct { int other; } jv;\n"),
        ("other/plain.c", "jv plain(jv value) { return value; }\n"),
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let related =
        |path: &str, name: &str| related_types_for(&conn, &workspace, &db_path, path, name);

    assert_related_owner(
        &related("src/jq.h", "jq_get_exit_code"),
        "jv",
        "src/jv.h",
        4,
    );
    assert_related_owner(&related("src/main.c", "run"), "jv", "src/jv.h", 4);
    assert_related_owner(
        &related("src/error.c", "where"),
        "position_t",
        "include/lib/detail/position.h",
        1,
    );
    assert_no_related(&related("other/plain.c", "plain"), "} jv");
}
