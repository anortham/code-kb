use code_kb_core::{
    SiteLabel, Workspace, open_read_only, reference_report, safe_tempdir, scan_workspace,
    select_reference_target,
};
use std::fs;

fn imported_call_is_proven(module: &str, name: &str) -> bool {
    reference_is_proven(&[("Helpers.psm1", module)], name, "Helpers.psm1")
}

fn reference_is_proven(files: &[(&str, &str)], name: &str, target: &str) -> bool {
    let temp = safe_tempdir();
    let root = temp.path();
    for (path, source) in files {
        fs::write(root.join(path), source).unwrap();
    }
    fs::write(
        root.join("run.ps1"),
        format!(
            "Import-Module \"$PSScriptRoot/Helpers.psm1\"\nfunction Invoke-Helper {{ {name} }}\n"
        ),
    )
    .unwrap();
    let db = root.join(".code-kb/artifact.db");
    scan_workspace(&Workspace::new(root.to_path_buf()), &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let symbol = select_reference_target(&conn, name, Some(target)).unwrap();
    let report = reference_report(&conn, root, &symbol, false).unwrap();
    let sites: Vec<_> = report
        .sites
        .iter()
        .filter(|site| site.site.path == "run.ps1" && site.site.start_line == Some(2))
        .collect();
    assert_eq!(sites.len(), 1, "{report:#?}");
    sites[0].label == SiteLabel::Proven
}

#[test]
fn module_functions_are_exported_by_default() {
    assert!(imported_call_is_proven(
        "function Get-Value { 1 }\n",
        "Get-Value"
    ));
}

#[test]
fn explicit_exports_do_not_prove_an_unlisted_function() {
    assert!(!imported_call_is_proven(
        "function Get-Value { 1 }\nfunction Get-Public { 2 }\nExport-ModuleMember -Function Get-Public\n",
        "Get-Value",
    ));
}

#[test]
fn an_empty_export_command_does_not_prove_a_function() {
    assert!(!imported_call_is_proven(
        "function Get-Value { 1 }\nExport-ModuleMember\n",
        "Get-Value",
    ));
}

#[test]
fn dynamic_exports_do_not_prove_a_function() {
    assert!(!imported_call_is_proven(
        "function Get-Value { 1 }\n$names = Get-Content exports.txt\nExport-ModuleMember -Function $names\n",
        "Get-Value",
    ));
}

#[test]
fn comma_separated_exports_and_export_all_keep_function_proof() {
    for exports in [
        "Export-ModuleMember -Function 'Get-Other', 'Get-Value'",
        "Export-ModuleMember -Function *",
    ] {
        assert!(
            imported_call_is_proven(
                &format!("function Get-Value {{ 1 }}\nfunction Get-Other {{ 2 }}\n{exports}\n"),
                "Get-Value",
            ),
            "{exports}"
        );
    }
}

#[test]
fn variable_exports_do_not_prove_a_function_with_the_same_name() {
    assert!(!imported_call_is_proven(
        "function Get-Value { 1 }\n${Get-Value} = 2\nExport-ModuleMember -Variable 'Get-Value'\n",
        "Get-Value",
    ));
}

#[test]
fn an_outer_module_export_boundary_applies_to_nested_modules() {
    assert!(!reference_is_proven(
        &[
            (
                "Helpers.psm1",
                "Import-Module \"$PSScriptRoot/Inner.psm1\"\nfunction Get-Public { 2 }\nExport-ModuleMember -Function Get-Public\n"
            ),
            ("Inner.psm1", "function Get-Value { 1 }\n"),
        ],
        "Get-Value",
        "Inner.psm1",
    ));
}

#[test]
fn an_outer_module_can_explicitly_reexport_a_nested_function() {
    assert!(reference_is_proven(
        &[
            (
                "Helpers.psm1",
                "Import-Module \"$PSScriptRoot/Inner.psm1\"\nExport-ModuleMember -Function Get-Value\n"
            ),
            ("Inner.psm1", "function Get-Value { 1 }\n"),
        ],
        "Get-Value",
        "Inner.psm1",
    ));
}
