use code_kb_core::{
    Workspace, compute_blast_radius, file_skeleton_op, find_julie_extract_binary,
    find_references_scoped, fts_search_symbols_scoped, open_read_only, open_read_write,
    safe_tempdir, scan_workspace, search_symbols_scoped,
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
            "possible: builds `App`, and a test client calls its `__call__`, which reaches the target; shares"
        ),
        "{reason}"
    );
    let reason = &result.likely_tests[position("test_user_error_is_handled")].reason;
    assert!(
        reason.starts_with(
            "possible: builds `App` through fixture `app`, and a test client calls its `__call__`, which reaches the target; shares"
        ),
        "{reason}"
    );
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
    ]);
    let conn = open_read_only(&db_path).unwrap();
    let mut rows: Vec<_> = ["__init__", "main", "Install"]
        .iter()
        .map(|name| {
            code_kb_core::get_symbol_by_name(&conn, name, None)
                .unwrap()
                .unwrap()
        })
        .collect();

    code_kb_core::qualify_members(&conn, rows.iter_mut()).unwrap();

    let names: Vec<&str> = rows.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["ScriptInfo.__init__", "main", "Install"]);
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
