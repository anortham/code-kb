use code_kb_core::{
    Workspace, find_julie_extract_binary, find_references_scoped, open_read_only, safe_tempdir,
    scan_workspace,
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
