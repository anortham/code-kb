use std::{fs, path::PathBuf};

use code_kb_core::{
    db::open_read_only,
    find_references_for_symbol,
    models::{LabeledSite, ReferenceReport, SiteLabel},
    ops::blast_radius_op,
    reference_report::{reference_report, reference_report_for_edit, select_reference_target},
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use rusqlite::{Connection, params};

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).expect("scan failed");
    Repo {
        root,
        db,
        _temp: temp,
    }
}

fn type_facts(conn: &Connection, path: &str) -> Vec<(String, String, String, String)> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(parent.name, ''), symbol.name, symbol.kind, fact.resolved_type
             FROM type_facts fact
             JOIN symbols symbol ON symbol.symbol_id = fact.symbol_id
             LEFT JOIN symbols parent ON parent.symbol_id = symbol.parent_symbol_id
             WHERE symbol.path = ?1
             ORDER BY symbol.start_line, symbol.start_column",
        )
        .unwrap();
    stmt.query_map(params![path], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })
    .unwrap()
    .collect::<Result<Vec<_>, _>>()
    .unwrap()
}

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path)).unwrap();
    let result = reference_report(&conn, &repo.root, &target, false);
    result.unwrap()
}

fn line_of(repo: &Repo, path: &str, needle: &str) -> usize {
    fs::read_to_string(repo.root.join(path))
        .unwrap()
        .lines()
        .position(|line| line.contains(needle))
        .map(|line| line + 1)
        .unwrap()
}

fn line_of_after(repo: &Repo, path: &str, needle: &str, after: usize) -> usize {
    fs::read_to_string(repo.root.join(path))
        .unwrap()
        .lines()
        .enumerate()
        .find(|(line, text)| *line + 1 > after && text.contains(needle))
        .map(|(line, _)| line + 1)
        .unwrap()
}

fn site_at<'a>(report: &'a ReferenceReport, path: &str, line: usize) -> &'a LabeledSite {
    report
        .sites
        .iter()
        .find(|site| site.site.path == path && site.site.start_line == Some(line))
        .unwrap_or_else(|| panic!("missing site at {path}:{line}: {:?}", report.sites))
}

#[test]
fn a_typed_field_receiver_chain_proves_its_service_and_excludes_a_decoy() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "service/mail.go",
            "package service\n\ntype MailService struct{}\n\nfunc (*MailService) Send() {}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport \"fx/service\"\n\ntype InvitationServiceGroup struct {\n\tservice *service.InvitationService\n}\n\ntype InvitationHandler struct {\n\tgroup *InvitationServiceGroup\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.group.service.Send()\n}\n",
        ),
        (
            "handler/mail.go",
            "package handler\n\nimport \"fx/service\"\n\ntype MailServiceGroup struct {\n\tservice *service.MailService\n}\n\ntype MailHandler struct {\n\tgroup *MailServiceGroup\n}\n\nfunc (h *MailHandler) ServeHTTP() {\n\th.group.service.Send()\n}\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    let facts = type_facts(&conn, "handler/invitation.go");
    assert!(
        facts.contains(&(
            String::from("InvitationServiceGroup"),
            String::from("service"),
            String::from("field"),
            String::from("service.InvitationService")
        )),
        "InvitationServiceGroup field fact missing: {facts:?}"
    );
    assert!(
        facts.iter().any(|(_, name, _, resolved_type)| {
            name == "h" && resolved_type == "InvitationHandler"
        }),
        "InvitationHandler receiver fact missing: {facts:?}"
    );
    drop(conn);

    let report = report(&repo, "InvitationService.Send", "service/invitation.go");
    let invitation_line = line_of(&repo, "handler/invitation.go", "h.group.service.Send()");
    let mail_line = line_of(&repo, "handler/mail.go", "h.group.service.Send()");

    assert_eq!(
        site_at(&report, "handler/mail.go", mail_line).label,
        SiteLabel::Excluded
    );
    let invitation = site_at(&report, "handler/invitation.go", invitation_line);
    assert_eq!(invitation.label, SiteLabel::Proven, "{invitation:?}");
}

#[test]
fn a_mixed_plain_and_nested_call_batch_keeps_both_evidence_paths() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service.go",
            "package app\n\ntype InvitationService struct{}\nfunc (*InvitationService) Send() {}\ntype Handler struct { service InvitationService }\nfunc (*Handler) Send() {}\nfunc (h *Handler) Invoke() {\n\th.service.Send()\n\th.Send()\n}\n",
        ),
    ]);
    let report = report(&repo, "InvitationService.Send", "service.go");
    let nested_line = line_of(&repo, "service.go", "h.service.Send()");
    let plain_line = line_of(&repo, "service.go", "h.Send()");

    assert_eq!(
        site_at(&report, "service.go", nested_line).label,
        SiteLabel::Proven
    );
    assert_eq!(
        site_at(&report, "service.go", plain_line).label,
        SiteLabel::Excluded
    );
}

#[test]
fn a_removed_target_keeps_typed_receiver_chain_evidence_after_rescan() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport \"fx/service\"\n\ntype InvitationHandler struct {\n\tservice *service.InvitationService\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.service.Send()\n}\n",
        ),
    ]);
    let target = {
        let conn = open_read_only(&repo.db).unwrap();
        select_reference_target(
            &conn,
            "InvitationService.Send",
            Some("service/invitation.go"),
        )
        .unwrap()
    };
    fs::write(
        repo.root.join("service/invitation.go"),
        "package service\n\ntype InvitationService struct{}\n",
    )
    .unwrap();
    scan_workspace(&Workspace::new(repo.root.clone()), &repo.db, true).expect("rescan failed");

    let conn = open_read_only(&repo.db).unwrap();
    let missing_target_count = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE symbol_id = ?1",
            params![target.symbol_id],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(missing_target_count, 0);
    let report = reference_report_for_edit(&conn, &repo.root, &target, None).unwrap();
    let line = line_of(&repo, "handler/invitation.go", "h.service.Send()");
    let site = site_at(&report, "handler/invitation.go", line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
}

#[test]
fn a_java_typed_field_receiver_chain_proves_its_service_and_excludes_a_decoy() {
    let repo = scanned_repo(&[(
        "G.java",
        r#"class InvitationService { void send() {} }
class MailService { void send() {} }
class InvitationHandler {
    InvitationService service;
    void handle(InvitationHandler h) { h.service.send(); }
}
class MailHandler {
    MailService service;
    void handle(MailHandler mail) { mail.service.send(); }
}
"#,
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let facts = type_facts(&conn, "G.java");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type.ends_with("InvitationService")
        }),
        "InvitationHandler field fact missing: {facts:?}"
    );
    assert!(
        facts.iter().any(|(_, name, _, resolved_type)| {
            name == "h" && resolved_type == "InvitationHandler"
        }),
        "InvitationHandler parameter fact missing: {facts:?}"
    );
    drop(conn);

    let report = report(&repo, "InvitationService.send", "G.java");
    let invitation_line = line_of(&repo, "G.java", "h.service.send();");
    let mail_line = line_of(&repo, "G.java", "mail.service.send();");

    assert_eq!(
        site_at(&report, "G.java", mail_line).label,
        SiteLabel::Excluded
    );
    let invitation = site_at(&report, "G.java", invitation_line);
    assert_eq!(invitation.label, SiteLabel::Proven, "{invitation:?}");
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "InvitationService.send", Some("G.java")).unwrap();
    let callers =
        find_references_for_symbol(&conn, &target.name, "callers", 20, &target.symbol_id).unwrap();
    assert!(
        callers
            .iter()
            .any(|site| { site.path == "G.java" && site.start_line == Some(invitation_line) })
    );
    assert!(
        !callers
            .iter()
            .any(|site| site.path == "G.java" && site.start_line == Some(mail_line))
    );
    let blast = blast_radius_op(
        &workspace,
        &conn,
        Some("InvitationService.send"),
        None,
        3,
        20,
    )
    .unwrap();
    assert!(
        blast
            .impacted_symbols
            .iter()
            .any(|symbol| { symbol.path == "G.java" && symbol.name == "handle" })
    );
}

#[test]
fn a_csharp_typed_field_receiver_chain_proves_its_service_and_excludes_a_decoy() {
    let repo = scanned_repo(&[(
        "G.cs",
        r#"public class InvitationService { public void Send() {} }
public class MailService { public void Send() {} }
public class InvitationHandler
{
    public InvitationService service;
    public void Handle(InvitationHandler h) { h.service.Send(); }
}
public class MailHandler
{
    public MailService service;
    public void Handle(MailHandler mail) { mail.service.Send(); }
}
"#,
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let facts = type_facts(&conn, "G.cs");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type.ends_with("InvitationService")
        }),
        "InvitationHandler field fact missing: {facts:?}"
    );
    assert!(
        facts.iter().any(|(_, name, _, resolved_type)| {
            name == "h" && resolved_type == "InvitationHandler"
        }),
        "InvitationHandler parameter fact missing: {facts:?}"
    );
    drop(conn);

    let report = report(&repo, "InvitationService.Send", "G.cs");
    let invitation_line = line_of(&repo, "G.cs", "h.service.Send();");
    let mail_line = line_of(&repo, "G.cs", "mail.service.Send();");

    let mail_site = site_at(&report, "G.cs", mail_line);
    assert_eq!(mail_site.label, SiteLabel::Excluded, "{mail_site:?}");
    let invitation = site_at(&report, "G.cs", invitation_line);
    assert_eq!(invitation.label, SiteLabel::Proven, "{invitation:?}");
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "InvitationService.Send", Some("G.cs")).unwrap();
    let callers =
        find_references_for_symbol(&conn, &target.name, "callers", 20, &target.symbol_id).unwrap();
    assert!(
        callers
            .iter()
            .any(|site| { site.path == "G.cs" && site.start_line == Some(invitation_line) })
    );
    assert!(
        !callers
            .iter()
            .any(|site| site.path == "G.cs" && site.start_line == Some(mail_line))
    );
    let blast = blast_radius_op(
        &workspace,
        &conn,
        Some("InvitationService.Send"),
        None,
        3,
        20,
    )
    .unwrap();
    assert!(
        blast
            .impacted_symbols
            .iter()
            .any(|symbol| { symbol.path == "G.cs" && symbol.name == "Handle" })
    );
}

#[test]
fn a_missing_intermediate_type_fact_does_not_exclude_a_nested_call() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport \"fx/service\"\n\ntype InvitationHandler struct {\n\tservice *service.InvitationService\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.service.Send()\n}\n",
        ),
    ]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "handler/invitation.go");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type.ends_with("InvitationService")
        }),
        "InvitationHandler field fact missing before simulation: {facts:?}"
    );

    let conn = Connection::open(&repo.db).unwrap();
    let removed = conn
        .execute(
            "DELETE FROM type_facts
             WHERE symbol_id = (
                 SELECT field.symbol_id FROM symbols field
                 JOIN symbols owner ON owner.symbol_id = field.parent_symbol_id
                 WHERE field.path = ?1 AND owner.name = 'InvitationHandler'
                   AND field.name = 'service' AND field.kind = 'field'
             )",
            params!["handler/invitation.go"],
        )
        .unwrap();
    assert_eq!(removed, 1);
    drop(conn);

    let report = report(&repo, "InvitationService.Send", "service/invitation.go");
    let line = line_of(&repo, "handler/invitation.go", "h.service.Send()");
    let site = site_at(&report, "handler/invitation.go", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "a missing intermediate type fact must stay unproven: {site:?}"
    );
}

#[test]
fn a_go_nested_receiver_chain_reaches_its_handler_and_test_in_blast_radius() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\nfunc PrepareInvitation() {}\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {\n\tPrepareInvitation()\n}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport \"fx/service\"\n\ntype InvitationHandler struct {\n\tservice *service.InvitationService\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.service.Send()\n}\n",
        ),
        (
            "handler/other.go",
            "package handler\n\ntype OtherHandler struct{}\n\nfunc (*OtherHandler) ServeHTTP() {}\n",
        ),
        (
            "handler/invitation_test.go",
            "package handler\n\nimport \"testing\"\n\nfunc TestServeHTTP(t *testing.T) {\n\tvar handler *InvitationHandler\n\thandler.ServeHTTP()\n\t(&InvitationHandler{}).ServeHTTP()\n}\n",
        ),
    ]);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let result = blast_radius_op(
        &workspace,
        &conn,
        Some("InvitationService.Send"),
        None,
        3,
        20,
    )
    .unwrap();
    let plain_seed_result =
        blast_radius_op(&workspace, &conn, Some("PrepareInvitation"), None, 3, 20).unwrap();
    let serve_report = report(
        &repo,
        "InvitationHandler.ServeHTTP",
        "handler/invitation.go",
    );
    let typed_line = line_of(&repo, "handler/invitation_test.go", "handler.ServeHTTP()");
    let composite_line = line_of(
        &repo,
        "handler/invitation_test.go",
        "(&InvitationHandler{}).ServeHTTP()",
    );
    assert_eq!(
        site_at(&serve_report, "handler/invitation_test.go", typed_line).label,
        SiteLabel::Proven
    );
    assert_eq!(
        site_at(&serve_report, "handler/invitation_test.go", composite_line).label,
        SiteLabel::Unresolved
    );

    assert!(
        result
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "handler/invitation.go" && symbol.name == "ServeHTTP"),
        "nested receiver caller missing from blast radius: {result:?}"
    );
    assert!(
        result
            .likely_tests
            .iter()
            .any(|test| test.path == "handler/invitation_test.go" && test.name == "TestServeHTTP"),
        "nested receiver caller test missing from blast radius: {result:?}"
    );
    assert!(
        plain_seed_result
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "service/invitation.go" && symbol.name == "Send"),
        "plain first hop missing from blast radius: {plain_seed_result:?}"
    );
    assert!(
        plain_seed_result
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "handler/invitation.go" && symbol.name == "ServeHTTP"),
        "typed second hop missing from blast radius: {plain_seed_result:?}"
    );
    assert!(
        plain_seed_result
            .likely_tests
            .iter()
            .any(|test| test.path == "handler/invitation_test.go" && test.name == "TestServeHTTP"),
        "plain third hop missing from blast radius: {plain_seed_result:?}"
    );
}

#[test]
fn a_shadowed_root_without_scope_facts_stays_unproven() {
    let repo = scanned_repo(&[(
        "service.go",
        "package app\n\ntype InvitationService struct{}\nfunc (*InvitationService) Send() {}\ntype MailService struct{}\nfunc (*MailService) Send() {}\ntype InvitationHandler struct { service *InvitationService }\ntype MailHandler struct { service *MailService }\nfunc (h *InvitationHandler) ServeHTTP() {\n\t{\n\t\th := &MailHandler{}\n\t\th.service.Send()\n\t}\n\th.service.Send()\n}\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "service.go");
    assert!(
        facts.iter().any(|(_, name, _, resolved_type)| {
            name == "h" && resolved_type == "InvitationHandler"
        }),
        "outer receiver fact missing: {facts:?}"
    );
    assert!(
        facts
            .iter()
            .any(|(_, name, _, resolved_type)| name == "h" && resolved_type == "MailHandler"),
        "shadowing local fact missing: {facts:?}"
    );

    let report = report(&repo, "InvitationService.Send", "service.go");
    let inner_line = line_of(&repo, "service.go", "h.service.Send()");
    let outer_line = line_of_after(&repo, "service.go", "h.service.Send()", inner_line);
    let inner = site_at(&report, "service.go", inner_line);
    assert_eq!(inner.label, SiteLabel::Unresolved, "{inner:?}");
    let outer = site_at(&report, "service.go", outer_line);
    assert_eq!(outer.label, SiteLabel::Unresolved, "{outer:?}");
}

#[test]
fn a_go_nested_chain_selects_duplicate_types_by_exact_import() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "first/invitation.go",
            "package first\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "second/invitation.go",
            "package second\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport service \"fx/second\"\n\ntype InvitationHandler struct {\n\tservice *service.InvitationService\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.service.Send()\n}\n",
        ),
    ]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "handler/invitation.go");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type.ends_with("InvitationService")
        }),
        "InvitationHandler field fact missing: {facts:?}"
    );

    let line = line_of(&repo, "handler/invitation.go", "h.service.Send()");
    let other = report(&repo, "InvitationService.Send", "first/invitation.go");
    let site = site_at(&other, "handler/invitation.go", line);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");

    let selected = report(&repo, "InvitationService.Send", "second/invitation.go");
    let site = site_at(&selected, "handler/invitation.go", line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
}

#[test]
fn conflicting_intermediate_type_facts_stay_unproven() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service/invitation.go",
            "package service\n\ntype InvitationService struct{}\n\nfunc (*InvitationService) Send() {}\n",
        ),
        (
            "service/mail.go",
            "package service\n\ntype MailService struct{}\n\nfunc (*MailService) Send() {}\n",
        ),
        (
            "handler/invitation.go",
            "package handler\n\nimport \"fx/service\"\n\ntype InvitationHandler struct {\n\tservice *service.InvitationService\n}\n\nfunc (h *InvitationHandler) ServeHTTP() {\n\th.service.Send()\n}\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    let field_id = conn
        .query_row(
            "SELECT field.symbol_id FROM symbols field
             JOIN symbols owner ON owner.symbol_id = field.parent_symbol_id
             WHERE field.path = ?1 AND owner.name = 'InvitationHandler'
               AND field.name = 'service' AND field.kind = 'field'",
            params!["handler/invitation.go"],
            |row| row.get::<_, String>(0),
        )
        .unwrap();
    drop(conn);

    let conn = Connection::open(&repo.db).unwrap();
    let inserted = conn
        .execute(
            "INSERT INTO type_facts
             (type_fact_id, symbol_id, language, resolved_type, generic_params_json, is_inferred)
             VALUES ('task8-conflicting-field-fact', ?1, 'go', 'service.MailService', NULL, 0)",
            params![field_id],
        )
        .unwrap();
    assert_eq!(inserted, 1);
    drop(conn);

    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "handler/invitation.go");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type == "service.InvitationService"
        }) && facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type == "service.MailService"
        }),
        "fixture does not contain conflicting field type facts: {facts:?}"
    );

    let report = report(&repo, "InvitationService.Send", "service/invitation.go");
    let line = line_of(&repo, "handler/invitation.go", "h.service.Send()");
    let site = site_at(&report, "handler/invitation.go", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "conflicting intermediate type facts must stay unproven: {site:?}"
    );
}

#[test]
fn a_dynamic_intermediate_field_does_not_exclude_a_nested_call() {
    let repo = scanned_repo(&[(
        "G.cs",
        r#"public class InvitationService { public void Send() {} }
public class InvitationHandler
{
    public dynamic service;
    public void Handle(InvitationHandler h) { h.service.Send(); }
}
"#,
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "G.cs");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler" && name == "service" && resolved_type == "dynamic"
        }),
        "dynamic intermediate fact missing: {facts:?}"
    );

    let report = report(&repo, "InvitationService.Send", "G.cs");
    let line = line_of(&repo, "G.cs", "h.service.Send();");
    let site = site_at(&report, "G.cs", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "a dynamic intermediate fact must stay unproven: {site:?}"
    );
}

#[test]
fn a_typed_chain_follows_an_explicit_js_base_import_past_a_nearer_decoy() {
    let repo = scanned_repo(&[
        ("src/lib/base.js", "export class Base { send() {} }\n"),
        ("src/app/base.ts", "export class Base { send() {} }\n"),
        (
            "src/app/derived.ts",
            "import { Base } from '../lib/base.js';\nclass Derived extends Base {}\nclass Handler { service: Derived; }\nfunction call(h: Handler) { h.service.send(); }\n",
        ),
    ]);
    let line = line_of(&repo, "src/app/derived.ts", "h.service.send()");
    let imported = report(&repo, "Base.send", "src/lib/base.js");
    let imported_site = site_at(&imported, "src/app/derived.ts", line);
    assert_eq!(imported_site.label, SiteLabel::Proven, "{imported_site:?}");
    let decoy = report(&repo, "Base.send", "src/app/base.ts");
    let decoy_site = site_at(&decoy, "src/app/derived.ts", line);
    assert_eq!(decoy_site.label, SiteLabel::Excluded, "{decoy_site:?}");

    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let imported_blast = blast_radius_op(
        &workspace,
        &conn,
        Some("Base.send"),
        Some("src/lib/base.js"),
        3,
        20,
    )
    .unwrap();
    assert!(
        imported_blast
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "src/app/derived.ts" && symbol.name == "call")
    );
    let decoy_blast = blast_radius_op(
        &workspace,
        &conn,
        Some("Base.send"),
        Some("src/app/base.ts"),
        3,
        20,
    )
    .unwrap();
    assert!(
        !decoy_blast
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "src/app/derived.ts" && symbol.name == "call")
    );
}

#[test]
fn equally_near_unresolved_bases_keep_both_receiver_sites_unproven() {
    let repo = scanned_repo(&[
        (
            "src/app/derived.ts",
            "class Derived extends Base {}\nclass Handler { service: Derived; }\nfunction call(h: Handler) { h.service.send(); }\n",
        ),
        ("src/app/left.ts", "class Base { send() {} }\n"),
        ("src/app/right.ts", "class Base { send() {} }\n"),
    ]);
    let line = line_of(&repo, "src/app/derived.ts", "h.service.send()");
    for path in ["src/app/left.ts", "src/app/right.ts"] {
        let report = report(&repo, "Base.send", path);
        let site = site_at(&report, "src/app/derived.ts", line);
        assert!(
            !matches!(site.label, SiteLabel::Proven | SiteLabel::Excluded),
            "ambiguous base ownership must stay unproven: {site:?}"
        );
    }
}

#[test]
fn unmatched_and_qualified_pending_bases_keep_receiver_sites_unproven() {
    let repo = scanned_repo(&[
        ("src/lib/base.js", "export class Base { send() {} }\n"),
        (
            "src/app/missing.ts",
            "import { Base } from './absent.js';\nclass MissingDerived extends Base {}\nclass MissingHandler { service: MissingDerived; }\nfunction missing(h: MissingHandler) { h.service.send(); }\n",
        ),
        (
            "src/app/qualified.ts",
            "class QualifiedDerived extends ns.Base {}\nclass QualifiedHandler { service: QualifiedDerived; }\nfunction qualified(h: QualifiedHandler) { h.service.send(); }\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    for (path, expected_namespace) in [
        ("src/app/missing.ts", "[]"),
        ("src/app/qualified.ts", "[\"ns\"]"),
    ] {
        let (target_name, namespace_json): (String, String) = conn
            .query_row(
                "SELECT pending.target_terminal_name, pending.target_namespace_json
                 FROM pending_relationships pending
                 JOIN symbols source ON source.symbol_id = pending.from_symbol_id
                 WHERE source.path = ?1 AND pending.kind = 'extends'",
                params![path],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(target_name, "Base");
        assert_eq!(namespace_json, expected_namespace);
    }
    let report = report(&repo, "Base.send", "src/lib/base.js");
    for path in ["src/app/missing.ts", "src/app/qualified.ts"] {
        let line = line_of(&repo, path, "h.service.send()");
        let site = site_at(&report, path, line);
        assert!(
            !matches!(site.label, SiteLabel::Proven | SiteLabel::Excluded),
            "unsupported pending ancestry must stay unproven: {site:?}"
        );
    }
}

#[test]
fn a_nested_receiver_chain_reaches_a_service_inherited_method() {
    let repo = scanned_repo(&[(
        "G.cs",
        r#"public class BaseService { public void Send() {} }
public class InvitationService : BaseService {}
public class InvitationHandler
{
    public InvitationService service;
    public void Handle(InvitationHandler h) { h.service.Send(); }
}
"#,
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "G.cs");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type == "InvitationService"
        }),
        "InvitationHandler field fact missing: {facts:?}"
    );

    let report = report(&repo, "BaseService.Send", "G.cs");
    let line = line_of(&repo, "G.cs", "h.service.Send();");
    let site = site_at(&report, "G.cs", line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
}

#[test]
fn a_nested_receiver_chain_prefers_the_nearest_service_override() {
    let repo = scanned_repo(&[(
        "G.cs",
        r#"public class BaseService { public virtual void Send() {} }
public class InvitationService : BaseService
{
    public override void Send() {}
}
public class InvitationHandler
{
    public InvitationService service;
    public void Handle(InvitationHandler h) { h.service.Send(); }
}
"#,
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "G.cs");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "InvitationHandler"
                && name == "service"
                && resolved_type == "InvitationService"
        }),
        "InvitationHandler field fact missing: {facts:?}"
    );
    let line = line_of(&repo, "G.cs", "h.service.Send();");

    let base = report(&repo, "BaseService.Send", "G.cs");
    let site = site_at(&base, "G.cs", line);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");

    let derived = report(&repo, "InvitationService.Send", "G.cs");
    let site = site_at(&derived, "G.cs", line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
}

#[test]
fn an_unsubstituted_generic_field_type_stays_unproven() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service.go",
            "package app\n\ntype InvitationService struct{}\nfunc (*InvitationService) Send() {}\ntype MailService struct{}\nfunc (*MailService) Send() {}\ntype T struct{}\nfunc (*T) Send() {}\ntype Wrapper[T any] struct { service T }\ntype Handler struct { group Wrapper[*InvitationService] }\nfunc (h *Handler) ServeHTTP() { h.group.service.Send() }\n",
        ),
    ]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "service.go");
    assert!(facts.iter().any(|(owner, name, _, resolved_type)| {
        owner == "Wrapper" && name == "service" && resolved_type == "T"
    }));

    let report = report(&repo, "InvitationService.Send", "service.go");
    let line = line_of(&repo, "service.go", "h.group.service.Send()");
    let site = site_at(&report, "service.go", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "generic field types must stay unproven: {site:?}"
    );
}

#[test]
fn an_enclosing_generic_type_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[(
        "G.java",
        "public class InvitationService { public void Send() {} }\nclass T { InvitationService service; }\nclass Caller<T> { void Handle(T h) { h.service.Send(); } }\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "G.java");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "Handle" && name == "h" && kind == "variable" && resolved_type == "T"
    }));
    let conn = open_read_only(&repo.db).unwrap();
    let owner_signature: String = conn
        .query_row(
            "SELECT owner.signature
             FROM symbols parameter
             JOIN type_facts fact ON fact.symbol_id = parameter.symbol_id
             JOIN symbols owner ON owner.symbol_id = parameter.parent_symbol_id
             WHERE parameter.path = 'G.java' AND parameter.name = 'h'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(owner_signature, "void Handle(T h)");

    let report = report(&repo, "InvitationService.Send", "G.java");
    let line = line_of(&repo, "G.java", "h.service.Send()");
    let site = site_at(&report, "G.java", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "an enclosing generic type parameter must stay unproven: {site:?}"
    );
}

#[test]
fn concrete_java_and_csharp_parameter_types_are_not_generic_parameters() {
    let repo = scanned_repo(&[
        (
            "java/Handler.java",
            "public class InvitationService { public void Send() {} }\nclass InvitationHandler { InvitationService service; }\nclass Caller { java.util.List<InvitationHandler> Handle(int count, InvitationHandler h) { h.service.Send(); return null; } }\n",
        ),
        (
            "csharp/Handler.cs",
            "public class InvitationService { public void Send() {} }\npublic class InvitationHandler { public InvitationService service; }\npublic class Caller { public System.Collections.Generic.List<InvitationHandler> Handle(int count, InvitationHandler h) { h.service.Send(); return null; } }\n",
        ),
    ]);

    for path in ["java/Handler.java", "csharp/Handler.cs"] {
        let facts = type_facts(&open_read_only(&repo.db).unwrap(), path);
        assert!(
            facts.iter().any(|(owner, name, kind, resolved_type)| {
                owner == "Handle"
                    && name == "h"
                    && kind == "variable"
                    && resolved_type == "InvitationHandler"
            }),
            "{path}: {facts:?}"
        );
        let conn = open_read_only(&repo.db).unwrap();
        let (owner_signature, owner_metadata, generic_params): (String, String, String) = conn
            .query_row(
                "SELECT owner.signature, COALESCE(owner.metadata_json, ''), COALESCE(fact.generic_params_json, '')
                 FROM symbols parameter
                 JOIN type_facts fact ON fact.symbol_id = parameter.symbol_id
                 JOIN symbols owner ON owner.symbol_id = parameter.parent_symbol_id
                 WHERE parameter.path = ?1 AND parameter.name = 'h' AND owner.name = 'Handle'",
                params![path],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert!(owner_signature.contains("List<InvitationHandler>"));
        assert!(owner_signature.contains(", InvitationHandler h"));
        assert!(!owner_metadata.contains("typeParameters"));
        assert!(generic_params.is_empty());

        let report = report(&repo, "InvitationService.Send", path);
        let line = line_of(&repo, path, "h.service.Send()");
        let site = site_at(&report, path, line);
        assert_eq!(site.label, SiteLabel::Proven, "{path}: {site:?}");
    }
}

#[test]
fn a_no_space_go_generic_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "service.go",
            "package app\n\ntype T struct{}\nfunc (*T) Send() {}\ntype Wrapper[U any,T any] struct { service T }\ntype Handler struct { group Wrapper[string,*InvitationService] }\ntype InvitationService struct{}\nfunc (*InvitationService) Send() {}\nfunc (h *Handler) ServeHTTP() { h.group.service.Send() }\n",
        ),
    ]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "service.go");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "Wrapper" && name == "service" && kind == "field" && resolved_type == "T"
    }));
    let conn = open_read_only(&repo.db).unwrap();
    let (owner_signature, generic_params): (String, String) = conn
        .query_row(
            "SELECT owner.signature, COALESCE(fact.generic_params_json, '')
             FROM symbols field
             JOIN type_facts fact ON fact.symbol_id = field.symbol_id
             JOIN symbols owner ON owner.symbol_id = field.parent_symbol_id
             WHERE field.path = 'service.go' AND field.name = 'service'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(owner_signature.contains("Wrapper[U any,T any]"));
    assert!(generic_params.is_empty());

    let report = report(&repo, "T.Send", "service.go");
    let line = line_of(&repo, "service.go", "h.group.service.Send()");
    let site = site_at(&report, "service.go", line);
    assert_eq!(site.label, SiteLabel::Unresolved, "{site:?}");
}

#[test]
fn a_java_prefix_generic_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[(
        "G.java",
        "public class InvitationService { public void Send() {} }\nclass Handler { InvitationService service; }\nclass T { InvitationService service; }\nclass Caller { static public <T extends Handler> void Handle(T h) { h.service.Send(); } }\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "G.java");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "Handle" && name == "h" && kind == "variable" && resolved_type == "T"
    }));
    let conn = open_read_only(&repo.db).unwrap();
    let owner_signature: String = conn
        .query_row(
            "SELECT owner.signature
             FROM symbols parameter
             JOIN type_facts fact ON fact.symbol_id = parameter.symbol_id
             JOIN symbols owner ON owner.symbol_id = parameter.parent_symbol_id
             WHERE parameter.path = 'G.java' AND parameter.name = 'h'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(owner_signature.contains("static public <T extends Handler>"));

    let report = report(&repo, "InvitationService.Send", "G.java");
    let line = line_of(&repo, "G.java", "h.service.Send()");
    let site = site_at(&report, "G.java", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "generic prefix types must stay unproven: {site:?}"
    );
}

#[test]
fn a_csharp_prefix_generic_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[(
        "G.cs",
        "class InvitationService { public void Send() {} }\nclass MailService { public void Send() {} }\nclass Handler { public InvitationService service; }\nclass TValue { public MailService service; }\nclass T<TArg> {}\nclass Caller { static T<TValue> T<TValue>(TValue h) where TValue : Handler { h.service.Send(); return null; } }\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let facts = type_facts(&conn, "G.cs");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "T" && name == "h" && kind == "variable" && resolved_type == "TValue"
    }));
    let owner_signature: String = conn
        .query_row(
            "SELECT owner.signature
             FROM symbols parameter
             JOIN type_facts fact ON fact.symbol_id = parameter.symbol_id
             JOIN symbols owner ON owner.symbol_id = parameter.parent_symbol_id
             WHERE parameter.path = 'G.cs' AND parameter.name = 'h'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(owner_signature.starts_with("static <TValue> T<TValue> T("));

    let report = report(&repo, "InvitationService.Send", "G.cs");
    let line = line_of(&repo, "G.cs", "h.service.Send()");
    let site = site_at(&report, "G.cs", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "C# prefix generic types must stay unproven: {site:?}"
    );
}

#[test]
fn a_scala_bracket_generic_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[(
        "src/Generic.scala",
        "class ConcreteT { def Send(): Unit = {} }\nclass Wrapper[ConcreteT] { var service: ConcreteT = _ }\nclass Caller { def Handle(h: Wrapper[ConcreteT]): Unit = { h.service.Send() } }\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "src/Generic.scala");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "Wrapper"
            && name == "service"
            && kind == "property"
            && resolved_type == "ConcreteT"
    }));

    let report = report(&repo, "ConcreteT.Send", "src/Generic.scala");
    let line = line_of(&repo, "src/Generic.scala", "h.service.Send()");
    let site = site_at(&report, "src/Generic.scala", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "Scala generic parameters must stay unproven: {site:?}"
    );
}

#[test]
fn a_unicode_generic_parameter_does_not_bind_a_concrete_same_name() {
    let repo = scanned_repo(&[(
        "src/Unicode.scala",
        "class Δ { def Send(): Unit = {} }\nclass UnicodeWrapper[Δ] { var service: Δ = _ }\nclass UnicodeCaller { def Handle(h: UnicodeWrapper[Δ]): Unit = { h.service.Send() } }\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "src/Unicode.scala");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "UnicodeWrapper" && name == "service" && kind == "property" && resolved_type == "Δ"
    }));

    let report = report(&repo, "Δ.Send", "src/Unicode.scala");
    let line = line_of(&repo, "src/Unicode.scala", "h.service.Send()");
    let site = site_at(&report, "src/Unicode.scala", line);
    assert!(
        matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
        "Unicode generic parameters must stay unproven: {site:?}"
    );
}

#[test]
fn an_owner_name_in_a_return_type_does_not_create_generic_parameters() {
    let repo = scanned_repo(&[(
        "src/Handler.cs",
        "public class Service { public void Send() {} }\npublic class TValue { public Service service; }\npublic class T<TArg> {}\npublic class Caller { public T<TValue> T(TValue h) { h.service.Send(); return null; } }\n",
    )]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "src/Handler.cs");
    assert!(facts.iter().any(|(owner, name, kind, resolved_type)| {
        owner == "T" && name == "h" && kind == "variable" && resolved_type == "TValue"
    }));

    let report = report(&repo, "Service.Send", "src/Handler.cs");
    let line = line_of(&repo, "src/Handler.cs", "h.service.Send()");
    let site = site_at(&report, "src/Handler.cs", line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
}

fn report_for_definition(repo: &Repo, name: &str, path: &str, line: usize) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let symbol_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND start_line = ?3",
            params![name, path, line as i64],
            |row| row.get(0),
        )
        .unwrap();
    let target = code_kb_core::queries::get_symbol_by_id(&conn, &symbol_id)
        .unwrap()
        .unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

const PYTHON_NESTED_TYPES: &str = "class Outer:\n    class Inner:\n        @staticmethod\n        def build():\n            return 1\n\n\nclass Other:\n    class Inner:\n        @staticmethod\n        def build():\n            return 2\n\n\ndef use():\n    return Outer.Inner.build()\n";

#[test]
fn a_python_nested_type_path_proves_its_method_and_excludes_a_same_file_decoy() {
    let repo = scanned_repo(&[("outer.py", PYTHON_NESTED_TYPES)]);
    let call = line_of(&repo, "outer.py", "Outer.Inner.build()");
    let outer = line_of(&repo, "outer.py", "return 1") - 1;
    let other = line_of(&repo, "outer.py", "return 2") - 1;

    let proven = report_for_definition(&repo, "build", "outer.py", outer);
    let site = site_at(&proven, "outer.py", call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "qualified path");

    let decoy = report_for_definition(&repo, "build", "outer.py", other);
    let site = site_at(&decoy, "outer.py", call);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");
}

#[test]
fn a_java_nested_type_path_proves_its_method_and_excludes_a_decoy() {
    let repo = scanned_repo(&[
        (
            "app/Outer.java",
            "package app;\n\npublic class Outer {\n    public static class Inner {\n        public static int build() { return 1; }\n    }\n}\n",
        ),
        (
            "app/Other.java",
            "package app;\n\npublic class Other {\n    public static class Inner {\n        public static int build() { return 2; }\n    }\n}\n",
        ),
        (
            "app/Use.java",
            "package app;\n\npublic class Use {\n    int run() { return Outer.Inner.build(); }\n}\n",
        ),
    ]);
    let call = line_of(&repo, "app/Use.java", "Outer.Inner.build()");

    let proven = report_for_definition(&repo, "build", "app/Outer.java", 5);
    let site = site_at(&proven, "app/Use.java", call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "qualified path");

    let decoy = report_for_definition(&repo, "build", "app/Other.java", 5);
    let site = site_at(&decoy, "app/Use.java", call);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");
}

#[test]
fn a_csharp_nested_type_path_proves_its_method_and_excludes_a_same_file_decoy() {
    let repo = scanned_repo(&[(
        "Outer.cs",
        "namespace App;\n\npublic class Outer {\n    public class Inner {\n        public static int Build() { return 1; }\n    }\n}\n\npublic class Other {\n    public class Inner {\n        public static int Build() { return 2; }\n    }\n}\n\npublic class Use {\n    int Run() { return Outer.Inner.Build(); }\n}\n",
    )]);
    let call = line_of(&repo, "Outer.cs", "Outer.Inner.Build()");
    let outer = line_of(&repo, "Outer.cs", "return 1;");
    let other = line_of(&repo, "Outer.cs", "return 2;");

    let proven = report_for_definition(&repo, "Build", "Outer.cs", outer);
    let site = site_at(&proven, "Outer.cs", call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "qualified path");

    let decoy = report_for_definition(&repo, "Build", "Outer.cs", other);
    let site = site_at(&decoy, "Outer.cs", call);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");
}

#[test]
fn a_java_package_qualified_nested_type_path_keeps_its_qualified_path_proof() {
    let repo = scanned_repo(&[
        (
            "app/Outer.java",
            "package app;\n\npublic class Outer {\n    public static class Inner {\n        public static int build() { return 1; }\n    }\n}\n",
        ),
        (
            "app/Other.java",
            "package app;\n\npublic class Other {\n    public static class Inner {\n        public static int build() { return 2; }\n    }\n}\n",
        ),
        (
            "use/Use.java",
            "package use;\n\npublic class Use {\n    int run() { return app.Outer.Inner.build(); }\n}\n",
        ),
    ]);
    let call = line_of(&repo, "use/Use.java", "app.Outer.Inner.build()");

    let proven = report_for_definition(&repo, "build", "app/Outer.java", 5);
    let site = site_at(&proven, "use/Use.java", call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "qualified path");

    let decoy = report_for_definition(&repo, "build", "app/Other.java", 5);
    let site = site_at(&decoy, "use/Use.java", call);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");
}

#[test]
fn an_untyped_class_attribute_after_a_type_root_stays_unproven() {
    let repo = scanned_repo(&[(
        "outer.py",
        "class Helper:\n    def run(self):\n        return 1\n\n\ndef make_helper():\n    return Helper()\n\n\nclass Outer:\n    helper = make_helper()\n\n    def run(self):\n        return 2\n\n\ndef use():\n    return Outer.helper.run()\n",
    )]);
    let call = line_of(&repo, "outer.py", "Outer.helper.run()");
    for line in [2, 13] {
        let report = report_for_definition(&repo, "run", "outer.py", line);
        let site = site_at(&report, "outer.py", call);
        assert!(
            matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
            "an untyped class attribute must keep the call unproven: {site:?}"
        );
    }
}

const PYTHON_SELF_SERVICES: &str = "class MailService:\n    def send(self):\n        return 1\n\n\nclass InviteService:\n    def send(self):\n        return 2\n\n\nclass MailHandler:\n    def __init__(self):\n        self.service = MailService()\n\n    def run(self):\n        return self.service.send()\n\n\nclass InviteHandler:\n    def __init__(self):\n        self.service: InviteService = InviteService()\n\n    def run(self):\n        return self.service.send() + 0\n";

#[test]
fn a_python_self_attribute_chain_proves_its_service_and_excludes_a_decoy() {
    let repo = scanned_repo(&[("app.py", PYTHON_SELF_SERVICES)]);
    let facts = type_facts(&open_read_only(&repo.db).unwrap(), "app.py");
    assert!(
        facts.iter().any(|(owner, name, _, resolved_type)| {
            owner == "MailHandler" && name == "service" && resolved_type == "MailService"
        }),
        "MailHandler attribute fact missing: {facts:?}"
    );
    let mail_call = line_of(&repo, "app.py", "self.service = MailService()") + 3;
    let invite_call = line_of(&repo, "app.py", "self.service.send() + 0");

    let mail = report(&repo, "MailService.send", "app.py");
    let site = site_at(&mail, "app.py", mail_call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "receiver type");
    assert_eq!(
        site_at(&mail, "app.py", invite_call).label,
        SiteLabel::Excluded
    );

    let invite = report(&repo, "InviteService.send", "app.py");
    let site = site_at(&invite, "app.py", invite_call);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(
        site_at(&invite, "app.py", mail_call).label,
        SiteLabel::Excluded
    );

    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "MailService.send", Some("app.py")).unwrap();
    let callers =
        find_references_for_symbol(&conn, &target.name, "callers", 20, &target.symbol_id).unwrap();
    assert!(
        callers
            .iter()
            .any(|site| site.path == "app.py" && site.start_line == Some(mail_call))
    );
    assert!(
        !callers
            .iter()
            .any(|site| site.path == "app.py" && site.start_line == Some(invite_call))
    );
}

#[test]
fn a_python_self_attribute_without_a_type_fact_stays_unproven() {
    let repo = scanned_repo(&[(
        "app.py",
        "class MailService:\n    def send(self):\n        return 1\n\n\nclass InviteService:\n    def send(self):\n        return 2\n\n\ndef make_service():\n    return MailService()\n\n\nclass MailHandler:\n    def __init__(self):\n        self.service = make_service()\n\n    def run(self):\n        return self.service.send()\n",
    )]);
    let call = line_of(&repo, "app.py", "self.service.send()");
    for name in ["MailService.send", "InviteService.send"] {
        let report = report(&repo, name, "app.py");
        let site = site_at(&report, "app.py", call);
        assert!(
            matches!(site.label, SiteLabel::Likely | SiteLabel::Unresolved),
            "an untyped self attribute must keep the call unproven: {site:?}"
        );
    }
}

#[test]
fn a_self_chain_in_a_nested_function_does_not_bind_the_enclosing_class() {
    let repo = scanned_repo(&[(
        "app.py",
        "class MailService:\n    def send(self):\n        return 1\n\n\nclass InviteService:\n    def send(self):\n        return 2\n\n\nclass MailHandler:\n    def __init__(self):\n        self.service = MailService()\n\n    def run(self):\n        def helper(self):\n            return self.service.send()\n        return helper(InviteHolder())\n\n\nclass InviteHolder:\n    def __init__(self):\n        self.service = InviteService()\n",
    )]);
    let call = line_of(&repo, "app.py", "return self.service.send()");
    let report = report(&repo, "InviteService.send", "app.py");
    let site = site_at(&report, "app.py", call);
    assert_ne!(site.label, SiteLabel::Excluded, "{site:?}");
}

fn assert_not_excluded(report: &ReferenceReport, path: &str, line: usize) {
    let site = site_at(report, path, line);
    assert_ne!(site.label, SiteLabel::Excluded, "{site:?}");
}

fn assert_excluded(report: &ReferenceReport, path: &str, line: usize) {
    let site = site_at(report, path, line);
    assert_eq!(site.label, SiteLabel::Excluded, "{site:?}");
}

fn assert_qualified_path(report: &ReferenceReport, path: &str, line: usize) {
    let site = site_at(report, path, line);
    assert_eq!(site.label, SiteLabel::Proven, "{site:?}");
    assert_eq!(site.basis, "qualified path");
}

#[test]
fn a_kotlin_type_path_does_not_exclude_a_companion_or_extension_member_of_its_type() {
    let repo = scanned_repo(&[(
        "Outer.kt",
        "package app\n\nclass Outer {\n    class Nested {\n        companion object {\n            fun build(): Int = 1\n        }\n    }\n}\n\nclass Other {\n    class Nested {\n        companion object {\n            fun build(): Int = 2\n            fun make(): Int = 2\n        }\n    }\n}\n\nfun Outer.Nested.Companion.make(): Int = 3\n\nfun use(): Int = Outer.Nested.build() + Outer.Nested.make()\n",
    )]);
    let call = line_of(&repo, "Outer.kt", "fun use()");

    let companion = report_for_definition(&repo, "build", "Outer.kt", 6);
    assert_not_excluded(&companion, "Outer.kt", call);
    let decoy = report_for_definition(&repo, "build", "Outer.kt", 14);
    assert_excluded(&decoy, "Outer.kt", call);

    let extension = report_for_definition(&repo, "make", "Outer.kt", 20);
    assert_not_excluded(&extension, "Outer.kt", call);
}

#[test]
fn a_csharp_type_path_does_not_exclude_a_member_of_another_partial_part() {
    let repo = scanned_repo(&[
        (
            "A.cs",
            "namespace App;\n\npublic partial class Outer {\n    public partial class Inner {\n        public static int Make() { return 0; }\n    }\n}\n\npublic class Use {\n    int Run() { return Outer.Inner.Build(); }\n}\n",
        ),
        (
            "B.cs",
            "namespace App;\n\npublic partial class Outer {\n    public partial class Inner {\n        public static int Build() { return 1; }\n    }\n}\n\npublic class Other {\n    public class Inner {\n        public static int Build() { return 2; }\n    }\n}\n",
        ),
    ]);
    let call = line_of(&repo, "A.cs", "Outer.Inner.Build()");

    let part = report_for_definition(&repo, "Build", "B.cs", 5);
    assert_not_excluded(&part, "A.cs", call);
    let decoy = report_for_definition(&repo, "Build", "B.cs", 11);
    assert_excluded(&decoy, "A.cs", call);
}

#[test]
fn a_swift_type_path_does_not_exclude_a_member_of_an_extension_of_its_type() {
    let repo = scanned_repo(&[(
        "Outer.swift",
        "struct Outer {\n    struct Inner {\n        static func make() -> Int { return 0 }\n    }\n}\n\nextension Outer.Inner {\n    static func build() -> Int { return 1 }\n}\n\nstruct Other {\n    struct Inner {\n        static func build() -> Int { return 2 }\n    }\n}\n\nfunc use() -> Int { return Outer.Inner.build() }\n",
    )]);
    let call = line_of(&repo, "Outer.swift", "Outer.Inner.build()");

    let extension = report_for_definition(&repo, "build", "Outer.swift", 8);
    assert_not_excluded(&extension, "Outer.swift", call);
    let decoy = report_for_definition(&repo, "build", "Outer.swift", 13);
    assert_excluded(&decoy, "Outer.swift", call);
}

#[test]
fn a_java_typed_constant_after_a_type_root_proves_its_type_and_excludes_a_decoy() {
    let repo = scanned_repo(&[
        (
            "app/Holder.java",
            "package app;\n\npublic class Holder {\n    public static final Holder INSTANCE = new Holder();\n    public int describe() { return 1; }\n}\n",
        ),
        (
            "app/Registry.java",
            "package app;\n\npublic class Registry {\n    public static final Registry INSTANCE = new Registry();\n    public int describe() { return 2; }\n}\n",
        ),
        (
            "app/Use.java",
            "package app;\n\npublic class Use {\n    int run() { return Holder.INSTANCE.describe(); }\n}\n",
        ),
    ]);
    let call = line_of(&repo, "app/Use.java", "Holder.INSTANCE.describe()");

    let holder = report_for_definition(&repo, "describe", "app/Holder.java", 5);
    assert_qualified_path(&holder, "app/Use.java", call);
    let decoy = report_for_definition(&repo, "describe", "app/Registry.java", 5);
    assert_excluded(&decoy, "app/Use.java", call);
}

#[test]
fn a_csharp_static_readonly_member_after_a_type_root_proves_its_type_and_excludes_a_decoy() {
    let repo = scanned_repo(&[(
        "Registry.cs",
        "namespace App;\n\npublic class Registry {\n    public static readonly Registry Default = new Registry();\n    public int Build() { return 1; }\n}\n\npublic class Factory {\n    public static readonly Factory Default = new Factory();\n    public int Build() { return 2; }\n}\n\npublic class Use {\n    int Run() { return Registry.Default.Build(); }\n}\n",
    )]);
    let call = line_of(&repo, "Registry.cs", "Registry.Default.Build()");

    let registry = report_for_definition(&repo, "Build", "Registry.cs", 5);
    assert_qualified_path(&registry, "Registry.cs", call);
    let decoy = report_for_definition(&repo, "Build", "Registry.cs", 10);
    assert_excluded(&decoy, "Registry.cs", call);
}

#[test]
fn a_java_enum_constant_after_a_type_root_proves_its_enum_and_keeps_a_constant_body_open() {
    let repo = scanned_repo(&[
        (
            "app/Color.java",
            "package app;\n\npublic enum Color {\n    RED,\n    BLUE {\n        @Override public int describe() { return 9; }\n    };\n    public int describe() { return 3; }\n}\n",
        ),
        (
            "app/Shade.java",
            "package app;\n\npublic enum Shade {\n    RED;\n    public int describe() { return 4; }\n}\n",
        ),
        (
            "app/Use.java",
            "package app;\n\npublic class Use {\n    int red() { return Color.RED.describe(); }\n    int blue() { return Color.BLUE.describe(); }\n}\n",
        ),
    ]);
    let red = line_of(&repo, "app/Use.java", "Color.RED.describe()");
    let blue = line_of(&repo, "app/Use.java", "Color.BLUE.describe()");

    let color = report_for_definition(&repo, "describe", "app/Color.java", 8);
    assert_qualified_path(&color, "app/Use.java", red);
    assert_not_excluded(&color, "app/Use.java", blue);
    let decoy = report_for_definition(&repo, "describe", "app/Shade.java", 5);
    assert_excluded(&decoy, "app/Use.java", red);
    let body = report_for_definition(&repo, "describe", "app/Color.java", 6);
    assert_not_excluded(&body, "app/Use.java", blue);
}

#[test]
fn a_python_enum_member_after_a_type_root_proves_its_enum_and_excludes_a_decoy() {
    let repo = scanned_repo(&[(
        "color.py",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n\n    def describe(self):\n        return 1\n\n\nclass Shade(Enum):\n    RED = 2\n\n    def describe(self):\n        return 2\n\n\ndef use():\n    return Color.RED.describe()\n",
    )]);
    let call = line_of(&repo, "color.py", "Color.RED.describe()");

    let color = report_for_definition(&repo, "describe", "color.py", 7);
    assert_qualified_path(&color, "color.py", call);
    let decoy = report_for_definition(&repo, "describe", "color.py", 14);
    assert_excluded(&decoy, "color.py", call);
}
