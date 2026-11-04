use code_kb_core::{Workspace, edit_check_op, open_read_only, safe_tempdir, scan_workspace};
use std::fs;
use std::path::Path;
use std::process::Command;

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn change_of(path: &str, before: &str, after: &str, name: &str) -> String {
    let temp = safe_tempdir();
    let root = temp.path();
    git(root, &["init", "-q", "-b", "main"]);
    fs::write(root.join(path), before).unwrap();
    git(root, &["add", path]);
    git(root, &["commit", "-q", "-m", "base"]);
    let workspace = Workspace::new(root.to_path_buf());
    let db = root.join(".code-kb/artifact.db");
    scan_workspace(&workspace, &db, true).unwrap();
    fs::write(root.join(path), after).unwrap();
    let conn = open_read_only(&db).unwrap();
    let report = edit_check_op(&workspace, &db, &conn, None).unwrap();
    report
        .changes
        .iter()
        .find(|change| change.display == name)
        .unwrap_or_else(|| panic!("missing {name}: {report:#?}"))
        .change
        .clone()
}

#[test]
fn a_rust_rename_survives_nested_block_comments() {
    let change = change_of(
        "lib.rs",
        "pub fn old_name(n: i32) -> i32 { n + 1 }\n",
        "pub fn new_name(n: i32) -> i32 { n /* outer /* inner */ end */ + 1 }\n",
        "old_name",
    );
    assert_eq!(change, "renamed to `new_name`");
}

#[test]
fn an_fsharp_rename_survives_block_comments() {
    let change = change_of(
        "sample.fs",
        "module Sample\nlet old_name n = n + 1\n",
        "module Sample\nlet new_name n = n (* note *) + 1\n",
        "Sample.old_name",
    );
    assert_eq!(change, "renamed to `new_name`");
}

#[test]
fn a_powershell_rename_survives_block_comments() {
    let change = change_of(
        "sample.ps1",
        "function Old-Name($n) { return $n + 1 }\n",
        "function New-Name($n) { return $n <# note #> + 1 }\n",
        "Old-Name",
    );
    assert_eq!(change, "renamed to `New-Name`");
}

#[test]
fn a_lua_rename_survives_long_bracket_comments() {
    let change = change_of(
        "sample.lua",
        "function old_name(n) return n + 1 end\n",
        "function new_name(n) return n --[=[ note ]=]\n + 1 end\n",
        "old_name",
    );
    assert_eq!(change, "renamed to `new_name`");
}

#[test]
fn a_csharp_rename_survives_comments_inside_interpolation_code() {
    let change = change_of(
        "Sample.cs",
        "public class Sample { public static string OldName(int n) { return $\"value: {n + 1}\"; } }\n",
        "public class Sample { public static string NewName(int n) { return $\"value: {n /* note */ + 1}\"; } }\n",
        "Sample.OldName",
    );
    assert_eq!(change, "renamed to `NewName`");
}

#[test]
fn different_literal_text_does_not_establish_a_rename() {
    let change = change_of(
        "lib.rs",
        "pub fn old_name() -> &'static str { \"http://old\" }\n",
        "pub fn new_name() -> &'static str { \"http://new\" }\n",
        "old_name",
    );
    assert_eq!(change, "removed");
}

#[test]
fn ordinary_formatting_keeps_a_rename_match() {
    let change = change_of(
        "lib.rs",
        "pub fn old_name(n: i32) -> i32 { n + 1 }\n",
        "pub fn new_name(n: i32) -> i32 {\n    n+1\n}\n",
        "old_name",
    );
    assert_eq!(change, "renamed to `new_name`");
}
