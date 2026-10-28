use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use code_kb_core::db::open_read_only;
use code_kb_core::formatters::format_edit_check;
use code_kb_core::ops::edit_check_op;
use code_kb_core::safe_tempdir;
use code_kb_core::sync::scan_workspace;
use code_kb_core::workspace::Workspace;
use tempfile::TempDir;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: TempDir,
}

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
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, content).unwrap();
}

fn committed_repo(files: &[(&str, &str)]) -> Repo {
    code_kb_core::sync::find_julie_extract_binary().expect("julie-extract binary must be present");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
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

fn check_of(repo: &Repo) -> code_kb_core::edit_check::EditCheck {
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    edit_check_op(&workspace, &repo.db, &conn, None).expect("edit check failed")
}

fn field_change<'a>(
    check: &'a code_kb_core::edit_check::EditCheck,
    path: &str,
) -> &'a code_kb_core::edit_check::ShapeChange {
    check
        .changes
        .iter()
        .find(|change| change.path == path && change.change.starts_with("fields"))
        .expect("field change missing")
}

fn has_site(change: &code_kb_core::edit_check::ShapeChange, path: &str, line: usize) -> bool {
    change
        .in_diff
        .iter()
        .chain(&change.proven)
        .chain(&change.likely)
        .any(|site| site.path == path && site.line == line)
}

#[test]
fn a_kotlin_data_class_reports_only_a_missing_required_primary_property() {
    let repo = committed_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
        (
            "shapes/Point.kt",
            "package shapes\ndata class Point(val x: Int, val y: Int)\nfun makePoint() = Point(1, 2)\n",
        ),
        (
            "shapes/DefaultPoint.kt",
            "package shapes\ndata class DefaultPoint(val x: Int, val y: Int)\nfun makeDefault() = DefaultPoint(1, 2)\n",
        ),
        (
            "shapes/CustomPoint.kt",
            "package shapes\ndata class CustomPoint(val x: Int, val y: Int) { constructor(x: Int): this(x, 0) }\nfun makeCustom() = CustomPoint(1)\n",
        ),
        (
            "other/Point.kt",
            "package other\ndata class Point(val x: Int, val y: Int)\nfun makeOther() = Point(1, 2)\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes/Point.kt",
        "package shapes\ndata class Point(val x: Int, val y: Int, val z: Int)\nfun makePoint() = Point(1, 2)\n",
    );
    write(
        &repo.root,
        "shapes/DefaultPoint.kt",
        "package shapes\ndata class DefaultPoint(val x: Int, val y: Int, val z: Int = 0)\nfun makeDefault() = DefaultPoint(1, 2)\n",
    );
    write(
        &repo.root,
        "shapes/CustomPoint.kt",
        "package shapes\ndata class CustomPoint(val x: Int, val y: Int, val z: Int) { constructor(x: Int): this(x, 0, 0) }\nfun makeCustom() = CustomPoint(1)\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let point = field_change(&check, "shapes/Point.kt");
    assert!(has_site(point, "shapes/Point.kt", 3), "{text}");
    assert!(!has_site(point, "other/Point.kt", 3), "{text}");
    for (path, line) in [("shapes/DefaultPoint.kt", 3), ("shapes/CustomPoint.kt", 3)] {
        let change = field_change(&check, path);
        assert!(!has_site(change, path, line), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

#[test]
fn a_scala_case_class_reports_only_a_missing_required_case_parameter() {
    let repo = committed_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
        (
            "shapes/Point.scala",
            "package shapes\ncase class Point(x: Int, y: Int)\nobject Use { val point = Point(1, 2) }\n",
        ),
        (
            "shapes/DefaultPoint.scala",
            "package shapes\ncase class DefaultPoint(x: Int, y: Int)\nobject Use { val point = DefaultPoint(1, 2) }\n",
        ),
        (
            "shapes/CustomPoint.scala",
            "package shapes\ncase class CustomPoint(x: Int, y: Int) { def this(x: Int) = this(x, 0) }\nobject Use { val point = CustomPoint(1) }\n",
        ),
        (
            "other/Point.scala",
            "package other\ncase class Point(x: Int, y: Int)\nobject Use { val point = Point(1, 2) }\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes/Point.scala",
        "package shapes\ncase class Point(x: Int, y: Int, z: Int)\nobject Use { val point = Point(1, 2) }\n",
    );
    write(
        &repo.root,
        "shapes/DefaultPoint.scala",
        "package shapes\ncase class DefaultPoint(x: Int, y: Int, z: Int = 0)\nobject Use { val point = DefaultPoint(1, 2) }\n",
    );
    write(
        &repo.root,
        "shapes/CustomPoint.scala",
        "package shapes\ncase class CustomPoint(x: Int, y: Int, z: Int) { def this(x: Int) = this(x, 0, 0) }\nobject Use { val point = CustomPoint(1) }\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let point = field_change(&check, "shapes/Point.scala");
    assert!(has_site(point, "shapes/Point.scala", 3), "{text}");
    assert!(!has_site(point, "other/Point.scala", 3), "{text}");
    for (path, line) in [
        ("shapes/DefaultPoint.scala", 3),
        ("shapes/CustomPoint.scala", 3),
    ] {
        let change = field_change(&check, path);
        assert!(!has_site(change, path, line), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

#[test]
fn a_csharp_positional_record_reports_only_a_missing_required_primary_property() {
    let repo = committed_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
        (
            "shapes/Point.cs",
            "namespace shapes;\npublic record Point(int X, int Y);\npublic class Use { Point point = new Point(1, 2); }\n",
        ),
        (
            "shapes/DefaultPoint.cs",
            "namespace shapes;\npublic record DefaultPoint(int X, int Y);\npublic class Use { DefaultPoint point = new DefaultPoint(1, 2); }\n",
        ),
        (
            "shapes/CustomPoint.cs",
            "namespace shapes;\npublic record CustomPoint(int X, int Y) { public CustomPoint(int X) : this(X, 0) {} }\npublic class Use { CustomPoint point = new CustomPoint(1); }\n",
        ),
        (
            "other/Point.cs",
            "namespace other;\npublic record Point(int X, int Y);\npublic class Use { Point point = new Point(1, 2); }\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes/Point.cs",
        "namespace shapes;\npublic record Point(int X, int Y, int Z);\npublic class Use { Point point = new Point(1, 2); }\n",
    );
    write(
        &repo.root,
        "shapes/DefaultPoint.cs",
        "namespace shapes;\npublic record DefaultPoint(int X, int Y, int Z = 0);\npublic class Use { DefaultPoint point = new DefaultPoint(1, 2); }\n",
    );
    write(
        &repo.root,
        "shapes/CustomPoint.cs",
        "namespace shapes;\npublic record CustomPoint(int X, int Y, int Z) { public CustomPoint(int X) : this(X, 0, 0) {} }\npublic class Use { CustomPoint point = new CustomPoint(1); }\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let point = field_change(&check, "shapes/Point.cs");
    assert!(has_site(point, "shapes/Point.cs", 3), "{text}");
    assert!(!has_site(point, "other/Point.cs", 3), "{text}");
    for (path, line) in [("shapes/DefaultPoint.cs", 3), ("shapes/CustomPoint.cs", 3)] {
        let change = field_change(&check, path);
        assert!(!has_site(change, path, line), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

#[test]
fn a_swift_memberwise_initializer_requires_only_new_properties_without_defaults() {
    let repo = committed_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
        (
            "Required.swift",
            "struct RequiredPoint { var x: Int; var y: Int }\nlet point = RequiredPoint(x: 1, y: 2)\n",
        ),
        (
            "Default.swift",
            "struct DefaultPoint { var x: Int; var y: Int }\nlet point = DefaultPoint(x: 1, y: 2)\n",
        ),
        (
            "Custom.swift",
            "struct CustomPoint { var x: Int; var y: Int; init(x: Int) { self.x = x; self.y = 0 } }\nlet point = CustomPoint(x: 1)\n",
        ),
    ]);
    write(
        &repo.root,
        "Required.swift",
        "struct RequiredPoint { var x: Int; var y: Int; var z: Int }\nlet point = RequiredPoint(x: 1, y: 2)\n",
    );
    write(
        &repo.root,
        "Default.swift",
        "struct DefaultPoint { var x: Int; var y: Int; var z: Int = 0; var note: String? }\nlet point = DefaultPoint(x: 1, y: 2)\n",
    );
    write(
        &repo.root,
        "Custom.swift",
        "struct CustomPoint { var x: Int; var y: Int; var z: Int; init(x: Int) { self.x = x; self.y = 0; self.z = 0 } }\nlet point = CustomPoint(x: 1)\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    assert!(
        has_site(field_change(&check, "Required.swift"), "Required.swift", 2),
        "{text}"
    );
    for (path, line) in [("Default.swift", 2), ("Custom.swift", 2)] {
        let change = field_change(&check, path);
        assert!(!has_site(change, path, line), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

#[test]
fn a_cpp_designated_initializer_reports_removed_members_but_accepts_additions_and_custom_constructors()
 {
    let repo = committed_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        ),
        (
            "Removed.cpp",
            "struct RemovedPoint { int x; int y; };\nRemovedPoint point{.x = 1};\n",
        ),
        (
            "Added.cpp",
            "struct AddedPoint { int x; };\nAddedPoint point{.x = 1};\n",
        ),
        (
            "Positional.cpp",
            "struct PositionalPoint { int x; int y; int z; };\nPositionalPoint point{1, 2, 3};\n",
        ),
        (
            "AddedPositional.cpp",
            "struct AddedPositionalPoint { int x; };\nAddedPositionalPoint point{1};\n",
        ),
        (
            "Custom.cpp",
            "struct CustomPoint { int x; int y; int z; CustomPoint(int first, int second, int third): x(first), y(second), z(third) {} };\nCustomPoint point{1, 2, 3};\n",
        ),
        (
            "Removed.c",
            "struct RemovedCPoint { int x; int y; };\nstruct RemovedCPoint point = {.x = 1};\n",
        ),
        (
            "Added.c",
            "struct AddedCPoint { int x; };\nstruct AddedCPoint point = {.x = 1};\n",
        ),
        (
            "Positional.c",
            "struct PositionalCPoint { int x; int y; int z; };\nstruct PositionalCPoint point = {1, 2, 3};\n",
        ),
        (
            "AddedPositional.c",
            "struct AddedPositionalCPoint { int x; };\nstruct AddedPositionalCPoint point = {1};\n",
        ),
    ]);
    write(
        &repo.root,
        "Removed.cpp",
        "struct RemovedPoint { int y; };\nRemovedPoint point{.x = 1};\n",
    );
    write(
        &repo.root,
        "Added.cpp",
        "struct AddedPoint { int x; int y; };\nAddedPoint point{.x = 1};\n",
    );
    write(
        &repo.root,
        "Custom.cpp",
        "struct CustomPoint { int y; int z; CustomPoint(int first, int second, int third): y(second), z(third) {} };\nCustomPoint point{1, 2, 3};\n",
    );
    write(
        &repo.root,
        "Positional.cpp",
        "struct PositionalPoint { int y; int z; };\nPositionalPoint point{1, 2, 3};\n",
    );
    write(
        &repo.root,
        "AddedPositional.cpp",
        "struct AddedPositionalPoint { int x; int y; };\nAddedPositionalPoint point{1};\n",
    );
    write(
        &repo.root,
        "Removed.c",
        "struct RemovedCPoint { int y; };\nstruct RemovedCPoint point = {.x = 1};\n",
    );
    write(
        &repo.root,
        "Positional.c",
        "struct PositionalCPoint { int y; int z; };\nstruct PositionalCPoint point = {1, 2, 3};\n",
    );
    write(
        &repo.root,
        "AddedPositional.c",
        "struct AddedPositionalCPoint { int x; int y; };\nstruct AddedPositionalCPoint point = {1};\n",
    );
    write(
        &repo.root,
        "Added.c",
        "struct AddedCPoint { int x; int y; };\nstruct AddedCPoint point = {.x = 1};\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    for path in ["Removed.cpp", "Removed.c", "Positional.cpp", "Positional.c"] {
        assert!(has_site(field_change(&check, path), path, 2), "{text}");
    }
    for path in [
        "Added.cpp",
        "Added.c",
        "AddedPositional.cpp",
        "AddedPositional.c",
        "Custom.cpp",
    ] {
        let change = field_change(&check, path);
        assert!(!has_site(change, path, 2), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

#[test]
fn a_zig_struct_requires_new_fields_unless_the_literal_supplies_them_or_the_field_has_a_default() {
    let repo = committed_repo(&[
        (
            "Required.zig",
            "const Point = struct { x: i32 };\nfn make() Point { return Point{ .x = 1 }; }\n",
        ),
        (
            "Default.zig",
            "const Point = struct { x: i32 };\nfn make() Point { return Point{ .x = 1 }; }\n",
        ),
    ]);
    write(
        &repo.root,
        "Required.zig",
        "const Point = struct { x: i32, y: i32 };\nfn make() Point { return Point{ .x = 1 }; }\n",
    );
    write(
        &repo.root,
        "Default.zig",
        "const Point = struct { x: i32, y: i32 = 0 };\nfn make() Point { return Point{ .x = 1 }; }\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    assert!(
        has_site(field_change(&check, "Required.zig"), "Required.zig", 2),
        "{text}"
    );
    let default = field_change(&check, "Default.zig");
    assert!(!has_site(default, "Default.zig", 2), "{text}");
    assert_eq!(default.unresolved, 0, "{text}");
}

#[test]
fn an_fsharp_typed_record_literal_needs_added_fields_but_a_copy_update_does_not() {
    let repo = committed_repo(&[(
        "Point.fs",
        "namespace Probe\ntype Point = { X: int }\nmodule Usage =\n    let missing : Point = { X = 1 }\n    let update (point: Point) = { point with X = 1 }\n",
    )]);
    write(
        &repo.root,
        "Point.fs",
        "namespace Probe\ntype Point = { X: int; Y: int }\nmodule Usage =\n    let missing : Point = { X = 1 }\n    let update (point: Point) = { point with X = 1 }\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let point = field_change(&check, "Point.fs");
    assert!(has_site(point, "Point.fs", 4), "{text}");
    assert!(!has_site(point, "Point.fs", 5), "{text}");
}

#[test]
fn an_elixir_struct_rejects_removed_keys_but_added_nil_fields_keep_old_literals_valid() {
    let repo = committed_repo(&[
        (
            "Removed.ex",
            "defmodule Probe.Removed do\n  defstruct [:x, :y]\nend\ndefmodule Probe.UseRemoved do\n  def make, do: %Probe.Removed{x: 1, y: 2}\nend\n",
        ),
        (
            "Added.ex",
            "defmodule Probe.Added do\n  defstruct [:x]\nend\ndefmodule Probe.UseAdded do\n  def make, do: %Probe.Added{x: 1}\nend\n",
        ),
    ]);
    write(
        &repo.root,
        "Removed.ex",
        "defmodule Probe.Removed do\n  defstruct [:x]\nend\ndefmodule Probe.UseRemoved do\n  def make, do: %Probe.Removed{x: 1, y: 2}\nend\n",
    );
    write(
        &repo.root,
        "Added.ex",
        "defmodule Probe.Added do\n  defstruct [:x, :y]\nend\ndefmodule Probe.UseAdded do\n  def make, do: %Probe.Added{x: 1}\nend\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    assert!(
        has_site(field_change(&check, "Removed.ex"), "Removed.ex", 5),
        "{text}"
    );
    let added = field_change(&check, "Added.ex");
    assert!(!has_site(added, "Added.ex", 5), "{text}");
    assert_eq!(added.unresolved, 0, "{text}");
}

#[test]
fn an_enforced_elixir_key_breaks_only_old_builds_that_omit_it() {
    let repo = committed_repo(&[(
        "Required.ex",
        "defmodule Probe.Required do\n  defstruct [:x]\nend\ndefmodule Probe.Use do\n  def make, do: %Probe.Required{x: 1}\n  def matches(value) do\n    %Probe.Required{x: x} = value\n    x\n  end\nend\n",
    )]);
    write(
        &repo.root,
        "Required.ex",
        "defmodule Probe.Required do\n  @enforce_keys [:y]\n  defstruct [:x, :y]\nend\ndefmodule Probe.Use do\n  def make, do: %Probe.Required{x: 1}\n  def make_valid, do: %Probe.Required{x: 1, y: 2}\n  def matches(value) do\n    %Probe.Required{x: x} = value\n    x\n  end\nend\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let change = field_change(&check, "Required.ex");
    assert!(has_site(change, "Required.ex", 6), "{text}");
    assert!(!has_site(change, "Required.ex", 7), "{text}");
    assert!(!has_site(change, "Required.ex", 9), "{text}");
}

#[test]
fn a_ruby_keyword_struct_reports_removed_keywords_but_not_added_nil_fields() {
    let repo = committed_repo(&[
        (
            "Removed.rb",
            "RemovedPoint = Struct.new(:x, :y, keyword_init: true)\ndef make_removed\n  RemovedPoint.new(x: 1, y: 2)\nend\n",
        ),
        (
            "Added.rb",
            "AddedPoint = Struct.new(:x, keyword_init: true)\ndef make_added\n  AddedPoint.new(x: 1)\nend\n",
        ),
    ]);
    write(
        &repo.root,
        "Removed.rb",
        "RemovedPoint = Struct.new(:x, keyword_init: true)\ndef make_removed\n  RemovedPoint.new(x: 1, y: 2)\nend\n",
    );
    write(
        &repo.root,
        "Added.rb",
        "AddedPoint = Struct.new(:x, :y, keyword_init: true)\ndef make_added\n  AddedPoint.new(x: 1)\nend\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    assert!(
        has_site(field_change(&check, "Removed.rb"), "Removed.rb", 3),
        "{text}"
    );
    let added = field_change(&check, "Added.rb");
    assert!(!has_site(added, "Added.rb", 3), "{text}");
    assert_eq!(added.unresolved, 0, "{text}");
}

#[test]
fn primary_record_calls_bind_required_named_parameters_after_defaults() {
    let repo = committed_repo(&[
        (
            "Required.kt",
            "data class Point(val x: Int = 0)\nfun make() = Point(x = 1)\n",
        ),
        (
            "Updated.kt",
            "data class Point(val x: Int = 0)\nfun make() = Point(x = 1)\n",
        ),
        (
            "Required.scala",
            "case class Point(x: Int = 0)\nobject Use { def make = Point(x = 1) }\n",
        ),
        (
            "Updated.scala",
            "case class Point(x: Int = 0)\nobject Use { def make = Point(x = 1) }\n",
        ),
        (
            "Required.cs",
            "public record Point(int X = 0);\npublic class Use { public Point Make() => new Point(X: 1); }\n",
        ),
        (
            "Updated.cs",
            "public record Point(int X = 0);\npublic class Use { public Point Make() => new Point(X: 1); }\n",
        ),
    ]);
    write(
        &repo.root,
        "Required.kt",
        "data class Point(val y: Int, val x: Int = 0)\nfun make() = Point(x = 1)\n",
    );
    write(
        &repo.root,
        "Updated.kt",
        "data class Point(val y: Int, val x: Int = 0)\nfun make() = Point(x = 1, y = 2)\n",
    );
    write(
        &repo.root,
        "Required.scala",
        "case class Point(y: Int, x: Int = 0)\nobject Use { def make = Point(x = 1) }\n",
    );
    write(
        &repo.root,
        "Updated.scala",
        "case class Point(y: Int, x: Int = 0)\nobject Use { def make = Point(x = 1, y = 2) }\n",
    );
    write(
        &repo.root,
        "Required.cs",
        "public record Point(int Y, int X = 0);\npublic class Use { public Point Make() => new Point(X: 1); }\n",
    );
    write(
        &repo.root,
        "Updated.cs",
        "public record Point(int Y, int X = 0);\npublic class Use { public Point Make() => new Point(X: 1, Y: 2); }\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    for (path, line) in [
        ("Required.kt", 2),
        ("Required.scala", 2),
        ("Required.cs", 2),
    ] {
        assert!(has_site(field_change(&check, path), path, line), "{text}");
    }
    for path in ["Updated.kt", "Updated.scala", "Updated.cs"] {
        let change = field_change(&check, path);
        assert!(change.proven.is_empty(), "{text}");
        assert!(change.likely.is_empty(), "{text}");
        assert_eq!(change.unresolved, 0, "{text}");
    }
}

fn proven_site(change: &code_kb_core::edit_check::ShapeChange, path: &str, line: usize) -> bool {
    change
        .proven
        .iter()
        .any(|site| site.path == path && site.line == line)
}

fn likely_site(change: &code_kb_core::edit_check::ShapeChange, path: &str, line: usize) -> bool {
    change
        .likely
        .iter()
        .any(|site| site.path == path && site.line == line)
}

#[test]
fn a_removed_python_attribute_read_through_a_typed_parameter_is_proven() {
    let repo = committed_repo(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/wrappers.py",
            "class Request:\n    url_rule: str | None = None\n    routing_exception: Exception | None = None\n",
        ),
        (
            "pkg/app.py",
            "from .wrappers import Request\n\n\ndef raise_routing(request: Request) -> None:\n    raise request.routing_exception\n\n\ndef dispatch(ctx) -> None:\n    req = ctx.request\n    if req.routing_exception is not None:\n        raise_routing(req)\n",
        ),
        (
            "pkg/external.py",
            "from werkzeug.wrappers import Request\n\n\ndef raw(request: Request) -> object:\n    return request.routing_exception\n",
        ),
    ]);
    write(
        &repo.root,
        "pkg/wrappers.py",
        "class Request:\n    url_rule: str | None = None\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let request = field_change(&check, "pkg/wrappers.py");
    assert!(proven_site(request, "pkg/app.py", 5), "{text}");
    assert!(likely_site(request, "pkg/app.py", 10), "{text}");
    assert!(!proven_site(request, "pkg/external.py", 5), "{text}");
}

#[test]
fn a_removed_java_field_read_through_a_typed_parameter_is_proven() {
    let repo = committed_repo(&[
        (
            "app/Point.java",
            "package app;\n\npublic class Point {\n    public int x;\n    public int y;\n}\n",
        ),
        (
            "app/Use.java",
            "package app;\n\npublic class Use {\n    public int read(Point p) {\n        return p.y;\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "app/Point.java",
        "package app;\n\npublic class Point {\n    public int x;\n}\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let point = field_change(&check, "app/Point.java");
    assert!(proven_site(point, "app/Use.java", 5), "{text}");
}

#[test]
fn a_removed_python_instance_attribute_read_through_self_is_proven() {
    let repo = committed_repo(&[(
        "tests/conftest.py",
        "class AuthActions:\n    def __init__(self, client):\n        self._client = client\n\n    def login(self):\n        return self._client.post(\"/login\")\n",
    )]);
    write(
        &repo.root,
        "tests/conftest.py",
        "class AuthActions:\n    def __init__(self, client):\n        pass\n\n    def login(self):\n        return self._client.post(\"/login\")\n",
    );

    let check = check_of(&repo);
    let text = format_edit_check(&check);
    let auth = field_change(&check, "tests/conftest.py");
    assert!(proven_site(auth, "tests/conftest.py", 6), "{text}");
}
