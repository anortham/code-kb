use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use code_kb_core::db::open_read_only;
use code_kb_core::formatters::{format_edit_check, format_reference_report};
use code_kb_core::models::{ReferenceReport, SiteLabel};
use code_kb_core::ops::edit_check_op;
use code_kb_core::queries::get_symbol_by_id;
use code_kb_core::reference_report::reference_report;
use code_kb_core::safe_tempdir;
use code_kb_core::sync::{find_julie_extract_binary, scan_workspace};
use code_kb_core::workspace::Workspace;
use rusqlite::params;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git must be present for edit-check tests");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, content).unwrap();
}

fn committed_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "--local", "user.name", "Code KB Tests"]);
    git(
        &root,
        &[
            "config",
            "--local",
            "user.email",
            "code-kb-tests@example.invalid",
        ],
    );
    for (path, content) in files {
        write(&root, path, content);
    }
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "base"]);
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).expect("scan failed");
    Repo {
        root,
        db,
        _temp: temp,
    }
}

fn edited_text(files: &[(&str, &str)], changed_path: &str, changed: &str) -> String {
    let repo = committed_repo(files);
    write(&repo.root, changed_path, changed);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let check = edit_check_op(&workspace, &repo.db, &conn, None).expect("edit check failed");
    format_edit_check(&check)
}

fn assert_parameter_range(language_path: &str, before: &str, after: &str, expected: &str) {
    let text = edited_text(&[(language_path, before)], language_path, after);
    assert!(text.contains(expected), "{text}");
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in files {
        write(&root, path, content);
    }
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).expect("scan failed");
    Repo {
        root,
        db,
        _temp: temp,
    }
}

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND kind NOT IN ('export', 'import') ORDER BY start_line LIMIT 1",
            params![name, path],
            |row| row.get(0),
        )
        .unwrap();
    let target = get_symbol_by_id(&conn, &id).unwrap().unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn site(report: &ReferenceReport, path: &str, line: usize) -> (SiteLabel, Option<u32>) {
    let found = report
        .sites
        .iter()
        .find(|site| site.site.path == path && site.site.start_line == Some(line))
        .unwrap_or_else(|| {
            panic!(
                "no site at {path}:{line}\n{}",
                format_reference_report(report, 20)
            )
        });
    (found.label, found.arg_count)
}

#[test]
fn php_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/lib.php",
            "<?php\nfunction scale($value, $factor) { return $value * $factor; }\nfunction named() { return scale(value: 1, factor: 2); }\n",
        )],
        "src/lib.php",
        "<?php\nfunction scale($value, $ratio) { return $value * $ratio; }\nfunction named() { return scale(value: 1, factor: 2); }\n",
    );
    assert!(
        text.contains("function `scale` (src/lib.php): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.php:3  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn ruby_keyword_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/lib.rb",
            "def scale(value, factor:)\n  value * factor\nend\ndef named\n  scale(1, factor: 2)\nend\n",
        )],
        "src/lib.rb",
        "def scale(value, ratio:)\n  value * ratio\nend\ndef named\n  scale(1, factor: 2)\nend\n",
    );
    assert!(
        text.contains("method `scale` (src/lib.rb): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.rb:5  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn ruby_new_required_keyword_arguments_report_only_calls_that_omit_them() {
    let text = edited_text(
        &[(
            "src/lib.rb",
            "def compute(x, *extra, kw:)\n  x\nend\ndef missing\n  compute(1, 2, kw: 3)\nend\ndef valid\n  compute(1, 2, kw: 3, z: 4)\nend\n",
        )],
        "src/lib.rb",
        "def compute(x, *extra, kw:, z:)\n  x\nend\ndef missing\n  compute(1, 2, kw: 3)\nend\ndef valid\n  compute(1, 2, kw: 3, z: 4)\nend\n",
    );
    assert!(text.contains("src/lib.rb:5  `missing`"), "{text}");
    assert!(!text.contains("src/lib.rb:9  `valid`"), "{text}");
}

#[test]
fn ruby_positional_parameter_renames_keep_the_call_contract() {
    let text = edited_text(
        &[(
            "src/lib.rb",
            "def scale(value)\n  value\nend\ndef named\n  scale(1)\nend\n",
        )],
        "src/lib.rb",
        "def scale(amount)\n  amount\nend\ndef named\n  scale(1)\nend\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
}

#[test]
fn scala_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Lib.scala",
            "object Lib {\n  def scale(value: Int, factor: Int): Int = value * factor\n  def named(): Int = scale(value = 1, factor = 2)\n}\n",
        )],
        "src/Lib.scala",
        "object Lib {\n  def scale(value: Int, ratio: Int): Int = value * ratio\n  def named(): Int = scale(value = 1, factor = 2)\n}\n",
    );
    assert!(
        text.contains("method `Lib.scale` (src/Lib.scala): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(text.contains("`named`  names parameter `factor`"), "{text}");
}

#[test]
fn dart_required_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/lib.dart",
            "int scale(int value, {required int factor}) => value * factor;\nint named() => scale(1, factor: 2);\n",
        )],
        "src/lib.dart",
        "int scale(int value, {required int ratio}) => value * ratio;\nint named() => scale(1, factor: 2);\n",
    );
    assert!(
        text.contains("function `scale` (src/lib.dart): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.dart:2  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn dart_positional_parameter_renames_keep_the_call_contract() {
    let text = edited_text(
        &[(
            "src/lib.dart",
            "int scale(int value) => value;\nint named() => scale(1);\n",
        )],
        "src/lib.dart",
        "int scale(int amount) => amount;\nint named() => scale(1);\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
}

#[test]
fn fsharp_method_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.fs",
            "type Scale() =\n    member _.Compute(value: int, factor: int) = value * factor\n    member this.Named() = this.Compute(value = 1, factor = 2)\n",
        )],
        "src/Scale.fs",
        "type Scale() =\n    member _.Compute(value: int, ratio: int) = value * ratio\n    member this.Named() = this.Compute(value = 1, factor = 2)\n",
    );
    assert!(
        text.contains("method `Scale.Compute` (src/Scale.fs): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/Scale.fs:3  `Named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn fsharp_constructor_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.fs",
            "type Scale(value: int, factor: int) =\n    member _.Value = value * factor\nlet named = Scale(value = 1, factor = 2)\n",
        )],
        "src/Scale.fs",
        "type Scale(value: int, ratio: int) =\n    member _.Value = value * ratio\nlet named = Scale(value = 1, factor = 2)\n",
    );
    assert!(text.contains("parameter renamed: factor → ratio"), "{text}");
    assert!(
        text.contains("src/Scale.fs:3  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn kotlin_primary_constructor_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.kt",
            "class Scale(value: Int, factor: Int) {\n    fun product() = value * factor\n}\nfun named() = Scale(value = 1, factor = 2)\n",
        )],
        "src/Scale.kt",
        "class Scale(value: Int, ratio: Int) {\n    fun product() = value * ratio\n}\nfun named() = Scale(value = 1, factor = 2)\n",
    );
    assert!(text.contains("parameter renamed: factor → ratio"), "{text}");
    assert!(
        text.contains("src/Scale.kt:4  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn kotlin_required_formal_after_a_default_reports_calls_that_leave_it_unbound() {
    let text = edited_text(
        &[(
            "src/Lib.kt",
            "fun compute(x: Int = 0): Int = x\nfun missingNamed() = compute(x = 1)\nfun missingPositional() = compute(1)\nfun validNamed() = compute(x = 1, y = 2)\nfun validPositional() = compute(1, 2)\n",
        )],
        "src/Lib.kt",
        "fun compute(x: Int = 0, y: Int): Int = x + y\nfun missingNamed() = compute(x = 1)\nfun missingPositional() = compute(1)\nfun validNamed() = compute(x = 1, y = 2)\nfun validPositional() = compute(1, 2)\n",
    );
    assert!(text.contains("src/Lib.kt:2  `missingNamed`"), "{text}");
    assert!(text.contains("src/Lib.kt:3  `missingPositional`"), "{text}");
    assert!(!text.contains("src/Lib.kt:4  `validNamed`"), "{text}");
    assert!(!text.contains("src/Lib.kt:5  `validPositional`"), "{text}");
}

#[test]
fn python_bound_and_unbound_method_calls_bind_receiver_positions_differently() {
    let text = edited_text(
        &[(
            "src/lib.py",
            "class Scale:\n    def compute(self, x=0):\n        return x\n    def bound(self):\n        return self.compute(1)\ndef unbound(scale):\n    return Scale.compute(scale)\n",
        )],
        "src/lib.py",
        "class Scale:\n    def compute(self, y, x=0):\n        return y + x\n    def bound(self):\n        return self.compute(1)\ndef unbound(scale):\n    return Scale.compute(scale)\n",
    );
    assert!(!text.contains("src/lib.py:5  `bound`"), "{text}");
    assert!(text.contains("src/lib.py:7  `unbound`"), "{text}");
}

#[test]
fn csharp_extension_methods_keep_explicit_and_bound_receiver_slots_distinct() {
    let text = edited_text(
        &[(
            "src/Scale.cs",
            "public class Scale {}\npublic static class Extensions { public static int Compute(this Scale scale, int x = 0) => x; }\npublic class Use { public int Bound(Scale scale) => scale.Compute(1); public int Unbound(Scale scale) => Extensions.Compute(scale); }\n",
        )],
        "src/Scale.cs",
        "public class Scale {}\npublic static class Extensions { public static int Compute(this Scale scale, int y, int x = 0) => y + x; }\npublic class Use { public int Bound(Scale scale) => scale.Compute(1); public int Unbound(Scale scale) => Extensions.Compute(scale); }\n",
    );
    assert!(!text.contains("src/Scale.cs:3  `Bound`"), "{text}");
    assert!(text.contains("src/Scale.cs:3  `Unbound`"), "{text}");
}

#[test]
fn typescript_this_parameters_do_not_consume_a_written_argument_slot() {
    let text = edited_text(
        &[(
            "src/lib.ts",
            "function compute(this: void, x?: number): number { return x ?? 0; }\nfunction caller() { return compute(1); }\n",
        )],
        "src/lib.ts",
        "function compute(this: void, y: number, x?: number): number { return y + (x ?? 0); }\nfunction caller() { return compute(1); }\n",
    );
    assert!(!text.contains("src/lib.ts:2  `caller`"), "{text}");
}

#[test]
fn scala_primary_constructor_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.scala",
            "class Scale(value: Int, factor: Int) { def product = value * factor }\nobject Use { val named = new Scale(value = 1, factor = 2) }\n",
        )],
        "src/Scale.scala",
        "class Scale(value: Int, ratio: Int) { def product = value * ratio }\nobject Use { val named = new Scale(value = 1, factor = 2) }\n",
    );
    assert!(text.contains("parameter renamed: factor → ratio"), "{text}");
    assert!(
        text.contains("src/Scale.scala:2  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn csharp_primary_constructor_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.cs",
            "public class Scale(int Value, int Factor) { public int Product => Value * Factor; }\npublic class Use { public Scale Named() => new Scale(Value: 1, Factor: 2); }\n",
        )],
        "src/Scale.cs",
        "public class Scale(int Value, int Ratio) { public int Product => Value * Ratio; }\npublic class Use { public Scale Named() => new Scale(Value: 1, Factor: 2); }\n",
    );
    assert!(text.contains("parameter renamed: Factor → Ratio"), "{text}");
    assert!(
        text.contains("src/Scale.cs:2  `Named`  names parameter `Factor`"),
        "{text}"
    );
}

#[test]
fn fsharp_primary_constructor_formals_have_an_exact_range() {
    let text = edited_text(
        &[(
            "src/Scale.fs",
            "type Scale(value: int) =\n    member _.Value = value\nlet named = Scale(1)\n",
        )],
        "src/Scale.fs",
        "type Scale(value: int, factor: int) =\n    member _.Value = value * factor\nlet named = Scale(1)\n",
    );
    assert!(text.contains("parameters 1 → 2"), "{text}");
    assert!(text.contains("src/Scale.fs:3  `named`"), "{text}");
}

#[test]
fn csharp_primary_constructor_formals_have_an_exact_range() {
    let text = edited_text(
        &[(
            "src/Scale.cs",
            "public class Scale(int Value) { public int Result => Value; }\npublic class Use { public Scale Named() => new Scale(1); }\n",
        )],
        "src/Scale.cs",
        "public class Scale(int Value, int Factor) { public int Result => Value * Factor; }\npublic class Use { public Scale Named() => new Scale(1); }\n",
    );
    assert!(text.contains("parameters 1 → 2"), "{text}");
    assert!(text.contains("src/Scale.cs:2  `Named`"), "{text}");
}

#[test]
fn vbnet_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/Scale.vb",
            "Public Class Scale\n    Public Shared Function Compute(value As Integer, factor As Integer) As Integer\n        Return value * factor\n    End Function\n    Public Shared Function Named() As Integer\n        Return Compute(value:=1, factor:=2)\n    End Function\nEnd Class\n",
        )],
        "src/Scale.vb",
        "Public Class Scale\n    Public Shared Function Compute(value As Integer, ratio As Integer) As Integer\n        Return value * ratio\n    End Function\n    Public Shared Function Named() As Integer\n        Return Compute(value:=1, factor:=2)\n    End Function\nEnd Class\n",
    );
    assert!(
        text.contains("method `Scale.Compute` (src/Scale.vb): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/Scale.vb:6  `Named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn vbnet_named_argument_matching_is_case_insensitive() {
    let text = edited_text(
        &[(
            "src/Scale.vb",
            "Public Class Scale\n    Public Shared Function Compute(value As Integer, factor As Integer) As Integer\n        Return value * factor\n    End Function\n    Public Shared Function Named() As Integer\n        Return Compute(value:=1, FACTOR:=2)\n    End Function\nEnd Class\n",
        )],
        "src/Scale.vb",
        "Public Class Scale\n    Public Shared Function Compute(value As Integer, ratio As Integer) As Integer\n        Return value * ratio\n    End Function\n    Public Shared Function Named() As Integer\n        Return Compute(value:=1, FACTOR:=2)\n    End Function\nEnd Class\n",
    );
    assert!(
        text.contains("src/Scale.vb:6  `Named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn swift_internal_parameter_renames_keep_unchanged_external_labels() {
    let text = edited_text(
        &[(
            "src/lib.swift",
            "func scale(from value: Int, by factor: Int) -> Int { value * factor }\nfunc named() -> Int { scale(from: 1, by: 2) }\n",
        )],
        "src/lib.swift",
        "func scale(from value: Int, by amount: Int) -> Int { value * amount }\nfunc named() -> Int { scale(from: 1, by: 2) }\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
}

#[test]
fn swift_external_label_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/lib.swift",
            "func scale(from value: Int, by factor: Int) -> Int { value * factor }\nfunc named() -> Int { scale(from: 1, by: 2) }\n",
        )],
        "src/lib.swift",
        "func scale(from value: Int, times factor: Int) -> Int { value * factor }\nfunc named() -> Int { scale(from: 1, by: 2) }\n",
    );
    assert!(
        text.contains("function `scale` (src/lib.swift): parameter renamed: by → times"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.swift:2  `named`  names parameter `by`"),
        "{text}"
    );
}

#[test]
fn python_positional_only_parameter_renames_keep_the_call_contract() {
    let text = edited_text(
        &[(
            "src/lib.py",
            "def scale(value, /):\n    return value\ndef named():\n    return scale(1)\n",
        )],
        "src/lib.py",
        "def scale(amount, /):\n    return amount\ndef named():\n    return scale(1)\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
}

#[test]
fn adding_a_cpp_defaulted_parameter_keeps_calls_inside_the_new_range() {
    let text = edited_text(
        &[(
            "src/lib.cpp",
            "int scale(int value) { return value; }\nint caller() { return scale(1); }\n",
        )],
        "src/lib.cpp",
        "int scale(int value, int factor = 1) { return value * factor; }\nint caller() { return scale(1); }\n",
    );
    assert!(text.contains("parameters 1 → 1-2"), "{text}");
    assert!(!text.contains("Not in your diff"), "{text}");
}

#[test]
fn adding_a_required_cpp_formal_keeps_variadic_calls_and_lists_too_few_arguments() {
    let text = edited_text(
        &[(
            "src/lib.cpp",
            "int sum(int first, ...) { return first; }\nint missing() { return sum(1); }\nint many() { return sum(1, 2, 3); }\n",
        )],
        "src/lib.cpp",
        "int sum(int first, int second, ...) { return first + second; }\nint missing() { return sum(1); }\nint many() { return sum(1, 2, 3); }\n",
    );
    assert!(text.contains("parameters 1 or more → 2 or more"), "{text}");
    assert!(
        text.contains("src/lib.cpp:2  `missing`  passes 1 arg; takes 2 or more"),
        "{text}"
    );
    assert!(!text.contains("src/lib.cpp:3  `many`"), "{text}");
}

#[test]
fn php_defaulted_formals_keep_existing_calls_safe() {
    let text = edited_text(
        &[(
            "src/lib.php",
            "<?php\nfunction scale($value) { return $value; }\nfunction caller() { return scale(1); }\n",
        )],
        "src/lib.php",
        "<?php\nfunction scale($value, $factor = 1) { return $value * $factor; }\nfunction caller() { return scale(1); }\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
    assert!(!text.contains("Not in your diff"), "{text}");
}

#[test]
fn php_variadic_formals_do_not_narrow_extra_argument_acceptance() {
    let text = edited_text(
        &[(
            "src/lib.php",
            "<?php\nfunction sum($first) { return $first; }\nfunction caller() { return sum(1, 2, 3); }\n",
        )],
        "src/lib.php",
        "<?php\nfunction sum($first, ...$rest) { return $first; }\nfunction caller() { return sum(1, 2, 3); }\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
    assert!(!text.contains("Not in your diff"), "{text}");
}

#[test]
fn php_typed_variadic_formals_have_an_open_ended_range() {
    let text = edited_text(
        &[(
            "src/lib.php",
            "<?php\nfunction compute(int $value, int ...$rest) { return $value; }\n$result = compute(1);\n",
        )],
        "src/lib.php",
        "<?php\nfunction compute(string $value, int ...$rest) { return $value; }\n$result = compute(1);\n",
    );
    assert!(!text.contains("passes 1 arg; takes 2"), "{text}");
}

#[test]
fn php_user_functions_may_receive_extra_positional_arguments() {
    let text = edited_text(
        &[(
            "functions.php",
            "<?php\nfunction compute($x, $y) { return func_get_args(); }\n$result = compute(1, 2);\n",
        )],
        "functions.php",
        "<?php\nfunction compute($x) { return func_get_args(); }\n$result = compute(1, 2);\n",
    );
    assert!(!text.contains("functions.php:3"), "{text}");
}

#[test]
fn ruby_defaulted_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/lib.rb",
        "def scale(value)\n value\nend\ndef caller\n scale(1)\nend\n",
        "def scale(value, factor = 1)\n value * factor\nend\ndef caller\n scale(1)\nend\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn ruby_defaulted_keyword_formals_remain_optional() {
    assert_parameter_range(
        "src/lib.rb",
        "def scale(value)\n  value\nend\ndef caller\n  scale(1)\nend\n",
        "def scale(value, factor: 1)\n  value * factor\nend\ndef caller\n  scale(1)\nend\ndef named\n  scale(1, factor: 2)\nend\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn ruby_required_trailing_formals_bind_positional_arguments_from_the_end() {
    let text = edited_text(
        &[(
            "src/lib.rb",
            "def compute(x = 0)\n  x\nend\ndef caller\n  compute(1)\nend\n",
        )],
        "src/lib.rb",
        "def compute(x = 0, y)\n  x + y\nend\ndef caller\n  compute(1)\nend\n",
    );
    assert!(!text.contains("src/lib.rb:5  `caller`"), "{text}");
}

#[test]
fn ruby_splat_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/lib.rb",
        "def sum(first)\n first\nend\n",
        "def sum(first, *rest)\n first\nend\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn scala_defaulted_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/Lib.scala",
        "object Lib { def scale(value: Int): Int = value; def caller(): Int = scale(1) }\n",
        "object Lib { def scale(value: Int, factor: Int = 1): Int = value * factor; def caller(): Int = scale(1) }\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn scala_repeated_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/Lib.scala",
        "object Lib { def sum(first: Int): Int = first }\n",
        "object Lib { def sum(first: Int, rest: Int*): Int = first }\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn dart_optional_positional_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/lib.dart",
        "int scale(int value) => value;\nint caller() => scale(1);\n",
        "int scale(int value, [int factor = 1]) => value * factor;\nint caller() => scale(1);\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn dart_named_formals_do_not_add_positional_arity() {
    let text = edited_text(
        &[(
            "src/lib.dart",
            "int scale(int value) => value;\nint caller() => scale(1);\n",
        )],
        "src/lib.dart",
        "int scale(int value, {String label = ''}) => value;\nint caller() => scale(1);\n",
    );
    assert!(text.contains("no symbol changed shape"), "{text}");
    assert!(!text.contains("Not in your diff"), "{text}");
}

#[test]
fn dart_new_required_named_formals_report_only_calls_that_omit_them() {
    let text = edited_text(
        &[(
            "src/lib.dart",
            "int scale(int value) => value;\nint missing() => scale(1);\nint valid() => scale(1);\n",
        )],
        "src/lib.dart",
        "int scale(int value, {required bool active}) => value;\nint missing() => scale(1);\nint valid() => scale(1, active: true);\n",
    );
    assert!(text.contains("src/lib.dart:2  `missing`"), "{text}");
    assert!(!text.contains("src/lib.dart:3  `valid`"), "{text}");
}

#[test]
fn dart_range_checks_count_only_positional_actuals() {
    let text = edited_text(
        &[(
            "src/lib.dart",
            "int scale(int value, {String label = '', bool active = false}) => value;\nint named() => scale(1, label: 'x', active: true);\nint positional() => scale(1, label: 'x', active: true);\n",
        )],
        "src/lib.dart",
        "int scale(int value, int factor, {String label = '', bool active = false}) => value;\nint named() => scale(1, label: 'x', active: true);\nint positional() => scale(1, 2, label: 'x', active: true);\n",
    );
    assert!(text.contains("parameters 1 → 2"), "{text}");
    assert!(text.contains("src/lib.dart:2  `named`"), "{text}");
    assert!(!text.contains("src/lib.dart:3  `positional`"), "{text}");
}

#[test]
fn swift_defaults_and_variadic_formals_widen_the_range() {
    assert_parameter_range(
        "src/lib.swift",
        "func scale(_ value: Int) -> Int { value }\nfunc caller() -> Int { scale(1) }\n",
        "func scale(_ value: Int, factor: Int = 1, values: Int...) -> Int { value }\nfunc caller() -> Int { scale(1) }\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn fsharp_optional_method_arguments_have_an_optional_range() {
    assert_parameter_range(
        "src/Scale.fs",
        "type Scale() =\n    member _.Value(value: int) = value\n    member this.Caller() = this.Value(1)\n",
        "type Scale() =\n    member _.Value(value: int, ?factor: int) = value\n    member this.Caller() = this.Value(1)\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn fsharp_param_array_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/Scale.fs",
        "type Scale() =\n    member _.Sum(first: int) = first\n",
        "type Scale() =\n    member _.Sum(first: int, [<System.ParamArray>] rest: int array) = first\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn vb_optional_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/Scale.vb",
        "Public Class Scale\n    Public Function Value(value As Integer) As Integer\n        Return value\n    End Function\n    Public Function Caller() As Integer\n        Return Value(1)\n    End Function\nEnd Class\n",
        "Public Class Scale\n    Public Function Value(value As Integer, Optional factor As Integer = 1) As Integer\n        Return value\n    End Function\n    Public Function Caller() As Integer\n        Return Value(1)\n    End Function\nEnd Class\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn vb_param_array_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/Scale.vb",
        "Public Class Scale\n    Public Function Sum(first As Integer) As Integer\n        Return first\n    End Function\nEnd Class\n",
        "Public Class Scale\n    Public Function Sum(first As Integer, ParamArray rest As Integer()) As Integer\n        Return first\n    End Function\nEnd Class\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn r_formals_may_be_missing_and_defaulted_formals_widen_the_maximum() {
    assert_parameter_range(
        "src/lib.R",
        "scale <- function(value) value\ncaller <- function() scale(1)\n",
        "scale <- function(value, factor = 1) value * factor\ncaller <- function() scale(1)\n",
        "parameters 0-1 → 0-2",
    );
}

#[test]
fn elixir_defaulted_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/lib.ex",
        "defmodule Lib do\n  def scale(value), do: value\n  def caller(), do: scale(1)\nend\n",
        "defmodule Lib do\n  def scale(value, factor \\\\ 1), do: value * factor\n  def caller(), do: scale(1)\nend\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn erlang_formal_arity_is_read_from_the_indexed_function_head() {
    assert_parameter_range(
        "src/lib.erl",
        "-module(lib).\n-export([scale/1, caller/0]).\nscale(A) -> A.\ncaller() -> scale(1).\n",
        "-module(lib).\n-export([scale/2, caller/0]).\nscale(A, B) -> A + B.\ncaller() -> scale(1).\n",
        "parameters 1 → 2",
    );
}

#[test]
fn zig_fixed_formals_have_an_exact_range() {
    assert_parameter_range(
        "src/lib.zig",
        "pub fn scale(value: i32) i32 { return value; }\npub fn caller() i32 { return scale(1); }\n",
        "pub fn scale(value: i32, factor: i32) i32 { return value * factor; }\npub fn caller() i32 { return scale(1); }\n",
        "parameters 1 → 2",
    );
}

#[test]
fn zig_c_abi_variadic_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/lib.zig",
        "extern fn printf(format: [*:0]const u8) c_int;\n",
        "extern fn printf(format: [*:0]const u8, ...) c_int;\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn gdscript_defaulted_formals_have_an_optional_range() {
    assert_parameter_range(
        "src/lib.gd",
        "func scale(value: int):\n    return value\nfunc caller():\n    return scale(1)\n",
        "func scale(value: int, factor: int = 1):\n    return value * factor\nfunc caller():\n    return scale(1)\n",
        "parameters 1 → 1-2",
    );
}

#[test]
fn gdscript_variadic_formals_have_an_open_ended_range() {
    assert_parameter_range(
        "src/lib.gd",
        "func sum(first: int):\n    return first\n",
        "func sum(first: int, ...rest):\n    return first\n",
        "parameters 1 → 1 or more",
    );
}

#[test]
fn r_named_parameter_renames_report_the_old_call() {
    let text = edited_text(
        &[(
            "src/lib.R",
            "scale <- function(value, factor) { value * factor }\nnamed <- function() scale(value = 1, factor = 2)\n",
        )],
        "src/lib.R",
        "scale <- function(value, ratio) { value * ratio }\nnamed <- function() scale(value = 1, factor = 2)\n",
    );
    assert!(
        text.contains("function `scale` (src/lib.R): parameter renamed: factor → ratio"),
        "{text}"
    );
    assert!(
        text.contains("src/lib.R:2  `named`  names parameter `factor`"),
        "{text}"
    );
}

#[test]
fn r_exact_named_formals_after_dots_keep_renames_external() {
    let text = edited_text(
        &[(
            "src/lib.R",
            "scale <- function(..., old) old\nnamed <- function() scale(old = 1)\n",
        )],
        "src/lib.R",
        "scale <- function(..., older) older\nnamed <- function() scale(old = 1)\n",
    );
    assert!(text.contains("parameter renamed: old → older"), "{text}");
    assert!(
        text.contains("src/lib.R:2  `named`  names parameter `old`"),
        "{text}"
    );
}

#[test]
fn r_old_parameter_prefixes_that_still_bind_are_not_stale() {
    let text = edited_text(
        &[(
            "src/lib.R",
            "scale <- function(old) old\ncaller <- function() scale(old = 1)\n",
        )],
        "src/lib.R",
        "scale <- function(older) older\ncaller <- function() scale(old = 1)\n",
    );
    assert!(text.contains("parameter renamed: old → older"), "{text}");
    assert!(
        !text.contains("src/lib.R:2  `caller`  names parameter `old`"),
        "{text}"
    );
}

#[test]
fn r_ranges_keep_missing_formals_permissive_and_honor_the_maximum_without_dots() {
    let text = edited_text(
        &[(
            "src/lib.R",
            "scale <- function(value, factor = 1, extra = 0) value * factor + extra\ncaller <- function() scale(1, 2, 3)\nmissing <- function() scale()\n",
        )],
        "src/lib.R",
        "scale <- function(value, factor = 1) value * factor\ncaller <- function() scale(1, 2, 3)\nmissing <- function() scale()\n",
    );
    assert!(text.contains("parameters 0-3 → 0-2"), "{text}");
    assert!(
        text.contains("src/lib.R:2  `caller`  passes 3 args; takes 0-2"),
        "{text}"
    );
    assert!(
        !text.contains("src/lib.R:3  `missing`  passes 0 args"),
        "{text}"
    );
}

#[test]
fn r_dots_make_the_accepted_argument_maximum_open_ended() {
    let text = edited_text(
        &[(
            "src/lib.R",
            "sum <- function(first) first\ncaller <- function() sum(1, 2, 3)\n",
        )],
        "src/lib.R",
        "sum <- function(first, ...) first\ncaller <- function() sum(1, 2, 3)\n",
    );
    assert!(text.contains("parameters 0-1 → 0 or more"), "{text}");
    assert!(!text.contains("src/lib.R:2  `caller`"), "{text}");
}

#[test]
fn c_unprototyped_declarations_leave_the_call_arity_unknown() {
    let repo = scanned_repo(&[
        ("src/api.h", "int f();\nint none(void);\n"),
        (
            "src/impl.c",
            "int f(int value) { return value; }\nint none(void) { return 0; }\n",
        ),
        (
            "src/use.c",
            "#include \"api.h\"\nint use(void) { return f(1); }\nint use_none(void) { return none(1); }\n",
        ),
        ("src/api.hpp", "int g();\n"),
        ("src/impl.cpp", "int g() { return 0; }\n"),
        (
            "src/use.cpp",
            "#include \"api.hpp\"\nint use() { return g(1); }\n",
        ),
    ]);
    let unprototyped = report(&repo, "f", "src/api.h");
    let exact_zero = report(&repo, "none", "src/api.h");
    let cpp_zero = report(&repo, "g", "src/api.hpp");
    assert_ne!(site(&unprototyped, "src/use.c", 2).0, SiteLabel::Excluded);
    assert_eq!(site(&exact_zero, "src/use.c", 3).0, SiteLabel::Excluded);
    assert_eq!(site(&cpp_zero, "src/use.cpp", 2).0, SiteLabel::Excluded);
}

#[test]
fn c_fixed_parameters_have_an_exact_call_range() {
    let text = edited_text(
        &[(
            "src/lib.c",
            "int scale(int value) { return value; }\nint caller(void) { return scale(1); }\n",
        )],
        "src/lib.c",
        "int scale(int value, int factor) { return value * factor; }\nint caller(void) { return scale(1); }\n",
    );
    assert!(text.contains("parameters 1 → 2"), "{text}");
    assert!(
        text.contains("src/lib.c:2  `caller`  passes 1 arg; takes 2"),
        "{text}"
    );
}

#[test]
fn powershell_reference_reports_command_parameter_calls_at_top_level_and_in_functions() {
    let repo = scanned_repo(&[(
        "src/lib.ps1",
        "function Send-Message { param([Parameter(Mandatory)][string]$Old) }\nSend-Message -Old hello\nfunction Invoke { Send-Message -Old goodbye }\n",
    )]);
    let report = report(&repo, "Send-Message", "src/lib.ps1");
    let (top_level, _) = site(&report, "src/lib.ps1", 2);
    assert_ne!(top_level, SiteLabel::Excluded);
    let (nested, _) = site(&report, "src/lib.ps1", 3);
    assert_ne!(nested, SiteLabel::Excluded);
}

#[test]
fn powershell_parameter_renames_report_command_calls() {
    let text = edited_text(
        &[(
            "src/lib.ps1",
            "function Send-Message { param([Parameter(Mandatory)][string]$Old) }\nSend-Message -Old hello\nfunction Invoke { Send-Message -Old goodbye }\n",
        )],
        "src/lib.ps1",
        "function Send-Message { param([Parameter(Mandatory)][string]$Message) }\nSend-Message -Old hello\nfunction Invoke { Send-Message -Old goodbye }\n",
    );
    assert!(
        text.contains("function `Send-Message` (src/lib.ps1): parameter renamed: Old → Message"),
        "{text}"
    );
    assert!(text.contains("src/lib.ps1:2"), "{text}");
    assert!(text.contains("src/lib.ps1:3"), "{text}");
}

#[test]
fn powershell_old_parameter_prefixes_that_still_bind_are_not_stale() {
    let text = edited_text(
        &[(
            "src/lib.ps1",
            "function Send-Message { param([Parameter(Mandatory)][string]$Old) }\nSend-Message -Old hello\n",
        )],
        "src/lib.ps1",
        "function Send-Message { param([Parameter(Mandatory)][string]$Older) }\nSend-Message -Old hello\n",
    );
    assert!(text.contains("parameter renamed: Old → Older"), "{text}");
    assert!(!text.contains("src/lib.ps1:2"), "{text}");
}
