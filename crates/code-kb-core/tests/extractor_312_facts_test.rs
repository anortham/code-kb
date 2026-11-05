use code_kb_core::{
    AccessCounts, ContextSlice, ReferenceReport, SiteAccess, SiteLabel, Symbol, Workspace,
    db::open_read_only,
    ops::{SymbolSelector, get_context_slice_selected_op, get_symbol_body_op},
    queries::get_symbol_by_name,
    reference_report, safe_tempdir, scan_workspace, select_reference_target,
    sync::find_julie_extract_binary,
};
use std::fs;
use std::path::PathBuf;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    workspace: Workspace,
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
    let workspace = Workspace::new(root.clone());
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&workspace, &db, true).expect("scan failed");
    Repo {
        root,
        db,
        workspace,
        _temp: temp,
    }
}

fn symbol(repo: &Repo, name: &str, path: &str) -> Symbol {
    let conn = open_read_only(&repo.db).unwrap();
    get_symbol_by_name(&conn, name, Some(path))
        .unwrap()
        .unwrap_or_else(|| panic!("{name} not indexed in {path}"))
}

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path)).unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn body(repo: &Repo, name: &str, path: &str) -> (Symbol, String) {
    let conn = open_read_only(&repo.db).unwrap();
    get_symbol_body_op(&repo.workspace, &repo.db, &conn, name, Some(path)).unwrap()
}

fn context(repo: &Repo, name: &str, path: &str) -> ContextSlice {
    let conn = open_read_only(&repo.db).unwrap();
    let target = symbol(repo, name, path);
    get_context_slice_selected_op(
        &repo.workspace,
        &repo.db,
        &conn,
        &SymbolSelector::Id(target.symbol_id),
        Some(path),
        false,
    )
    .unwrap()
}

fn line_of(repo: &Repo, path: &str, needle: &str) -> usize {
    fs::read_to_string(repo.root.join(path))
        .unwrap()
        .lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in {path}"))
        + 1
}

#[test]
fn erlang_recovers_the_function_after_a_macro_catch_head() {
    let source = r#"-module(p).
-export([f/0, g/0]).
-define(WITH_STACKTRACE(C, R, S), C:R:S ->).

f() ->
    try
        erlang:error(boom)
    catch
        ?WITH_STACKTRACE(C, R, S)
            io:format("~p~p~p", [C, R, S])
    end.

g() -> ok.
"#;
    let repo = scanned_repo(&[("src/recovered.erl", source)]);

    let (g, g_body) = body(&repo, "g", "src/recovered.erl");
    assert_eq!(g.signature.as_deref(), Some("g/0()"));
    assert!(g_body.contains("g() -> ok."), "{g_body}");
}

#[test]
fn powershell_script_root_imports_resolve_exported_module_functions() {
    let helper = r#"function Get-Public { 'public' }
function Get-Internal { 'internal' }
Export-ModuleMember -Function 'Get-Public'
"#;
    let caller = r#"Import-Module "$PSScriptRoot\..\Helpers.psm1" -Force
function Invoke-Public { Get-Public }
function Invoke-Internal { Get-Internal }
"#;
    let repo = scanned_repo(&[("Helpers.psm1", helper), ("Scripts/run.ps1", caller)]);

    let public_report = report(&repo, "Get-Public", "Helpers.psm1");
    let public_line = line_of(&repo, "Scripts/run.ps1", "Invoke-Public { Get-Public }");
    assert!(
        public_report.sites.iter().any(|site| {
            site.site.path == "Scripts/run.ps1"
                && site.site.start_line == Some(public_line)
                && site.label == SiteLabel::Proven
        }),
        "{public_report:#?}"
    );

    let internal_report = report(&repo, "Get-Internal", "Helpers.psm1");
    let internal_line = line_of(&repo, "Scripts/run.ps1", "Invoke-Internal { Get-Internal }");
    assert!(
        !internal_report.sites.iter().any(|site| {
            site.site.path == "Scripts/run.ps1"
                && site.site.start_line == Some(internal_line)
                && site.label == SiteLabel::Proven
        }),
        "{internal_report:#?}"
    );
}

#[test]
fn powershell_reports_occurrence_level_variable_access() {
    let source = r#"function Update-Count {
    $count = 0
    $count = 2
    $count += 1
    Write-Output $count
}
"#;
    let repo = scanned_repo(&[("Scripts/count.ps1", source)]);
    let report = report(&repo, "count", "Scripts/count.ps1");
    let accesses: Vec<(usize, Option<SiteAccess>)> = report
        .sites
        .iter()
        .filter(|site| site.label == SiteLabel::Proven)
        .map(|site| (site.site.start_line.unwrap(), site.access))
        .collect();

    assert_eq!(
        accesses,
        vec![
            (
                line_of(&repo, "Scripts/count.ps1", "$count = 2"),
                Some(SiteAccess::Write)
            ),
            (
                line_of(&repo, "Scripts/count.ps1", "$count += 1"),
                Some(SiteAccess::ReadWrite)
            ),
            (
                line_of(&repo, "Scripts/count.ps1", "Write-Output $count"),
                Some(SiteAccess::Read)
            ),
        ]
    );
    assert_eq!(
        report.access_counts,
        Some(AccessCounts {
            writes: 1,
            read_writes: 1,
            reads: 1,
            unclassified: 0,
        })
    );
}

#[test]
fn tsql_routines_keep_signatures_bodies_and_local_variable_access() {
    let source = r#"CREATE PROCEDURE dbo.p @Id int WITH EXECUTE AS OWNER AS BEGIN SELECT @Id; END;

CREATE FUNCTION dbo.GenerateDateDimensionColumns(@Year int)
RETURNS TABLE
AS
RETURN SELECT CASE
    WHEN EXISTS (SELECT 1 FROM dbo.T WHERE Id = @Year) THEN @Year
    ELSE 0
END AS Value;

CREATE FUNCTION [Application].DetermineCustomerAccess(@CityID int)
RETURNS TABLE
WITH SCHEMABINDING
AS
RETURN (
    SELECT 1 AS AccessResult
    FROM dbo.T
    WHERE dbo.T.Id = @CityID
);

CREATE FUNCTION [DataLoadSimulation].[GetAreaCode](@PhoneNumber nvarchar(20))
RETURNS NVARCHAR(4)
WITH EXECUTE AS OWNER
AS BEGIN
    DECLARE @AreaCode AS NVARCHAR(4);
    SELECT @AreaCode = SUBSTRING(@PhoneNumber, 1, 4);
    RETURN @AreaCode;
END;

CREATE PROCEDURE ChangeBalance @balance INT AS
BEGIN
    SET @balance = 1;
    SET @balance += 1;
    SELECT @balance;
END;

CREATE TABLE dbo.SequenceWords ([Next] int, OrderReference int);
SELECT [Next] FROM dbo.SequenceWords;
SELECT OrderReference, NEXT VALUE FOR Sequences.OrderID FROM dbo.SequenceWords;
"#;
    let repo = scanned_repo(&[("source.sql", source)]);

    let (procedure, procedure_body) = body(&repo, "p", "source.sql");
    assert!(procedure.signature.as_deref().unwrap().contains("@Id int"));
    assert!(
        procedure
            .signature
            .as_deref()
            .unwrap()
            .contains("WITH EXECUTE AS OWNER")
    );
    assert!(procedure_body.contains("SELECT @Id"), "{procedure_body}");

    let (tvf, tvf_body) = body(&repo, "GenerateDateDimensionColumns", "source.sql");
    assert!(tvf.signature.as_deref().unwrap().contains("@Year int"));
    assert!(tvf.signature.as_deref().unwrap().contains("RETURNS TABLE"));
    assert!(tvf_body.contains("RETURN SELECT CASE"), "{tvf_body}");

    let balance = report(&repo, "@balance", "source.sql");
    let access_rows: Vec<(usize, Option<SiteAccess>)> = balance
        .sites
        .iter()
        .filter(|site| site.label == SiteLabel::Proven)
        .map(|site| (site.site.start_line.unwrap(), site.access))
        .collect();
    assert_eq!(
        access_rows,
        vec![
            (
                line_of(&repo, "source.sql", "SET @balance = 1"),
                Some(SiteAccess::Write)
            ),
            (
                line_of(&repo, "source.sql", "SET @balance += 1"),
                Some(SiteAccess::ReadWrite)
            ),
            (
                line_of(&repo, "source.sql", "SELECT @balance"),
                Some(SiteAccess::Read)
            ),
        ]
    );
    assert_eq!(
        balance.access_counts,
        Some(AccessCounts {
            writes: 1,
            read_writes: 1,
            reads: 1,
            unclassified: 0,
        })
    );

    let next = report(&repo, "Next", "source.sql");
    let next_column_line = line_of(&repo, "source.sql", "SELECT [Next] FROM dbo.SequenceWords");
    let sequence_line = line_of(&repo, "source.sql", "NEXT VALUE FOR Sequences.OrderID");
    assert!(
        next.sites.iter().any(|site| {
            site.site.start_line == Some(next_column_line)
                && site.access == Some(SiteAccess::Read)
                && site.label != SiteLabel::Excluded
        }),
        "{next:#?}"
    );
    assert!(
        !next
            .sites
            .iter()
            .any(|site| { site.site.start_line == Some(sequence_line) }),
        "sequence keyword became a reference site: {next:#?}"
    );
}

#[test]
fn vbnet_parameter_attributes_keep_their_parameter_owner_out_of_method_conventions() {
    let source = r#"Class AccessibilityParameter
    <Obsolete>
    Public Function Check(<[In]> <Out> ByRef value As Integer) As Boolean
        Return value >= 0
    End Function

    <Obsolete>
    Public Sub Single(<Out> ByRef result As Integer, unmarked As String)
    End Sub

    Public Sub Plain(required As Integer, Optional note As String = Nothing)
    End Sub
End Class
"#;
    let repo = scanned_repo(&[("src/Parameters.vb", source)]);
    let check = symbol(&repo, "Check", "src/Parameters.vb");
    let value = symbol(&repo, "value", "src/Parameters.vb");
    assert_eq!(
        value.parent_symbol_id.as_deref(),
        Some(check.symbol_id.as_str())
    );
    assert!(
        value
            .signature
            .as_deref()
            .unwrap()
            .contains("<[In]> <Out> ByRef value As Integer")
    );

    let plain = context(&repo, "Plain", "src/Parameters.vb");
    let conventions: Vec<(String, usize, usize, bool)> = plain
        .conventions
        .iter()
        .map(|convention| {
            (
                convention.text.clone(),
                convention.siblings_with,
                convention.siblings,
                convention.target_has,
            )
        })
        .collect();
    assert_eq!(conventions, vec![("Obsolete".to_string(), 2, 2, false)]);
}
