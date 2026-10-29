use code_kb_core::{
    BaseRequest, BaseSource, EditCheck, EditLimits, OpError, Workspace, edit_check_op,
    edit_check_session_op, edit_check_with, ensure_fresh_file, find_julie_extract_binary,
    format_edit_check, format_stop_reason, head_commit, open_read_only, record_session_base,
    safe_tempdir, scan_workspace,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(["-c", "core.autocrlf=false", "-c", "commit.gpgsign=false"])
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args(args)
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("git {args:?}: {error}"));
    assert!(
        output.status.success(),
        "git {args:?}: {}",
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

fn run_check(repo: &Repo, base: Option<&str>) -> EditCheck {
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    edit_check_op(&workspace, &repo.db, &conn, base).expect("edit check failed")
}

fn text_of(repo: &Repo) -> String {
    format_edit_check(&run_check(repo, None))
}

const CARGO_TOML: &str = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n";

fn assert_has(text: &str, needle: &str) {
    assert!(text.contains(needle), "missing `{needle}` in:\n{text}");
}

fn assert_lacks(text: &str, needle: &str) {
    assert!(!text.contains(needle), "unexpected `{needle}` in:\n{text}");
}

fn rust_function_repo() -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub mod geometry;\npub mod first;\npub mod second;\n",
        ),
        (
            "src/geometry.rs",
            "pub fn area(w: i32, h: i32) -> i32 {\n    w * h\n}\n",
        ),
        (
            "src/first.rs",
            "use crate::geometry::area;\n\npub fn first() -> i32 {\n    area(2, 3)\n}\n",
        ),
        (
            "src/second.rs",
            "use crate::geometry::area;\n\npub fn second() -> i32 {\n    area(4, 5)\n}\n",
        ),
    ])
}

fn add_depth_parameter(repo: &Repo) {
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32, depth: i32) -> i32 {\n    w * h * depth\n}\n",
    );
    write(
        &repo.root,
        "src/first.rs",
        "use crate::geometry::area;\n\npub fn first() -> i32 {\n    area(2, 3, 1)\n}\n",
    );
}

#[test]
fn a_rust_function_that_gains_a_parameter_lists_the_caller_the_diff_did_not_reach() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let text = text_of(&repo);
    assert_has(&text, "function `area` (src/geometry.rs): parameters 2 → 3");
    assert_has(&text, "In your diff (1): src/first.rs:4");
    assert_has(&text, "Not in your diff, ");
    assert_has(&text, "src/second.rs:4  `second`  passes 2 args; takes 3");
    assert_lacks(&text, "src/first.rs:4  `first`");
}

#[test]
fn the_check_header_names_the_base_and_how_many_files_changed() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let text = text_of(&repo);
    assert!(
        text.starts_with(&format!(
            "check_edit: working tree against {} (HEAD), 2 files changed\n1 shape change:\n",
            &head[..10]
        )),
        "{text}"
    );
    assert!(
        text.ends_with("Not checked: calls inside macro arguments, callback invocation contracts. Indexed callable values are advisory dependencies.\n")
    );
}

#[test]
fn a_call_that_already_passes_the_new_count_is_not_listed() {
    let repo = committed_repo(&[
        ("a.py", "def render(x):\n    return x\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old():\n    return render(1)\n",
        ),
    ]);
    write(&repo.root, "a.py", "def render(x, y=2):\n    return x\n");
    let text = text_of(&repo);
    assert_has(&text, "function `render` (a.py): parameters 1 → 1-2");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_python_call_omitting_a_new_required_keyword_only_parameter_is_listed() {
    let repo = committed_repo(&[
        ("a.py", "def render(default=True):\n    return default\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old():\n    return render(False)\n",
        ),
    ]);
    write(
        &repo.root,
        "a.py",
        "def render(default=True, *, strict):\n    return default\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `render` (a.py): parameters 0-1 → 1-2");
    assert_has(&text, "b.py:5");
    assert_has(&text, "strict");
}

#[test]
fn a_touched_python_call_that_still_omits_a_new_required_keyword_is_not_in_the_diff() {
    let repo = committed_repo(&[
        ("a.py", "def render(default=True):\n    return default\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old():\n    return render(False)\n",
        ),
    ]);
    write(
        &repo.root,
        "a.py",
        "def render(default=True, *, strict):\n    return default\n",
    );
    write(
        &repo.root,
        "b.py",
        "from a import render\n\n\ndef old():\n    return render(default=False)\n",
    );
    let text = text_of(&repo);
    assert_lacks(&text, "In your diff");
    assert_has(&text, "Not in your diff");
    assert_has(&text, "missing required argument: strict");
}

#[test]
fn a_python_call_that_names_a_new_required_keyword_only_parameter_is_not_listed() {
    let repo = committed_repo(&[
        ("a.py", "def render(default=True):\n    return default\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old():\n    return render(False, strict=True)\n",
        ),
    ]);
    write(
        &repo.root,
        "a.py",
        "def render(default=True, *, strict):\n    return default\n",
    );
    let check = run_check(&repo, None);
    let change = &check.changes[0];
    assert!(change.in_diff.is_empty());
    assert!(change.proven.is_empty());
    assert!(change.likely.is_empty());
    assert_eq!(change.unresolved, 0);
}

#[test]
fn a_python_optional_keyword_only_parameter_does_not_list_positional_calls() {
    let repo = committed_repo(&[
        ("a.py", "def render(default=True):\n    return default\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old():\n    return render(False)\n",
        ),
    ]);
    write(
        &repo.root,
        "a.py",
        "def render(default=True, *, strict=True):\n    return default\n",
    );
    let check = run_check(&repo, None);
    let change = &check.changes[0];
    assert!(change.in_diff.is_empty());
    assert!(change.proven.is_empty());
    assert!(change.likely.is_empty());
    assert_eq!(change.unresolved, 0);
}

#[test]
fn a_python_unknown_keyword_spread_does_not_make_a_definite_missing_keyword_verdict() {
    let repo = committed_repo(&[
        ("a.py", "def render(default=True):\n    return default\n"),
        (
            "b.py",
            "from a import render\n\n\ndef old(kwargs):\n    return render(False, **kwargs)\n",
        ),
    ]);
    write(
        &repo.root,
        "a.py",
        "def render(default=True, *, strict):\n    return default\n",
    );
    let check = run_check(&repo, None);
    let change = &check.changes[0];
    assert!(change.in_diff.is_empty());
    assert!(change.proven.is_empty());
    assert!(change.likely.is_empty());
    assert_eq!(change.unresolved, 1);
}

#[test]
fn a_rust_struct_that_gains_a_field_lists_the_construction_the_diff_did_not_reach() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod shapes;\npub mod make;\n"),
        (
            "src/shapes.rs",
            "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n",
        ),
        (
            "src/make.rs",
            "use crate::shapes::Point;\n\npub fn origin() -> Point {\n    Point { x: 0, y: 0 }\n}\n\npub fn unit() -> Point {\n    Point { x: 1, y: 1 }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/shapes.rs",
        "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n    pub z: i32,\n}\n",
    );
    write(
        &repo.root,
        "src/make.rs",
        "use crate::shapes::Point;\n\npub fn origin() -> Point {\n    Point { x: 0, y: 0, z: 0 }\n}\n\npub fn unit() -> Point {\n    Point { x: 1, y: 1 }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Point` (src/shapes.rs): fields +1 (z), -0");
    assert_has(&text, "In your diff (1): src/make.rs:4");
    assert_has(&text, "src/make.rs:8");
    assert_has(&text, "`unit`");
    assert_lacks(&text, "src/make.rs:4  ");
}

#[test]
fn a_rust_enum_that_gains_a_variant_lists_the_function_with_the_match() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod color;\npub mod paint;\n"),
        (
            "src/color.rs",
            "pub enum Color {\n    Red,\n    Green,\n}\n",
        ),
        (
            "src/paint.rs",
            "use crate::color::Color;\n\npub fn name(c: Color) -> &'static str {\n    match c {\n        Color::Red => \"red\",\n        Color::Green => \"green\",\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/color.rs",
        "pub enum Color {\n    Red,\n    Green,\n    Blue,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (src/color.rs): variants +1 (Blue), -0");
    assert_has(&text, "Not in your diff, proven (1):");
    assert_has(&text, "src/paint.rs:5  `name`");
}

#[test]
fn a_function_that_names_the_new_variant_in_the_diff_is_not_listed() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod color;\npub mod paint;\n"),
        (
            "src/color.rs",
            "pub enum Color {\n    Red,\n    Green,\n}\n",
        ),
        (
            "src/paint.rs",
            "use crate::color::Color;\n\npub fn name(c: Color) -> &'static str {\n    match c {\n        Color::Red => \"red\",\n        Color::Green => \"green\",\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/color.rs",
        "pub enum Color {\n    Red,\n    Green,\n    Blue,\n}\n",
    );
    write(
        &repo.root,
        "src/paint.rs",
        "use crate::color::Color;\n\npub fn name(c: Color) -> &'static str {\n    match c {\n        Color::Red => \"red\",\n        Color::Green => \"green\",\n        Color::Blue => \"blue\",\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (src/color.rs): variants +1 (Blue), -0");
    assert_has(&text, "In your diff (1): src/paint.rs");
    assert_lacks(&text, "Not in your diff");
}

fn rust_color_repo(paint: &str) -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod color;\npub mod paint;\n"),
        (
            "src/color.rs",
            "pub enum Color {\n    Red,\n    Green,\n}\n",
        ),
        ("src/paint.rs", paint),
    ])
}

fn add_blue(repo: &Repo) -> String {
    write(
        &repo.root,
        "src/color.rs",
        "pub enum Color {\n    Red,\n    Green,\n    Blue,\n}\n",
    );
    text_of(repo)
}

#[test]
fn a_rust_match_with_a_wildcard_arm_is_counted_as_unresolved_and_not_listed() {
    let repo = rust_color_repo(
        "use crate::color::Color;\n\npub fn name(c: Color) -> &'static str {\n    match c {\n        Color::Red => \"red\",\n        _ => \"other\",\n    }\n}\n",
    );
    let text = add_blue(&repo);
    assert_has(&text, "enum `Color` (src/color.rs): variants +1 (Blue), -0");
    assert_has(&text, "Unresolved outside the diff: 1");
    assert_lacks(&text, "Not in your diff");
    assert_lacks(&text, "`name`");
}

#[test]
fn a_guarded_arm_that_names_a_variant_still_counts_but_a_guarded_wildcard_does_not() {
    let repo = rust_color_repo(
        "use crate::color::Color;\n\npub fn guarded(c: Color, ok: bool) -> i32 {\n    match c {\n        Color::Red if ok => 1,\n        Color::Red => 2,\n        Color::Green => 3,\n    }\n}\n\npub fn wildcard(c: Color, ok: bool) -> i32 {\n    match c {\n        Color::Red => 1,\n        _ if ok => 2,\n        Color::Green => 3,\n    }\n}\n\npub fn bound(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n        other => other as i32,\n    }\n}\n",
    );
    let text = add_blue(&repo);
    assert_has(&text, "src/paint.rs:5  `guarded`");
    assert_lacks(&text, "`wildcard`");
    assert_lacks(&text, "`bound`");
    assert_has(&text, "Unresolved outside the diff: 2");
}

#[test]
fn a_function_that_only_builds_or_tests_a_variant_is_counted_as_unresolved_and_not_listed() {
    let repo = rust_color_repo(
        "use crate::color::Color;\n\npub fn make() -> Color {\n    Color::Red\n}\n\npub fn is_red(c: &Color) -> bool {\n    matches!(c, Color::Red)\n}\n\npub fn if_let(c: Color) -> i32 {\n    if let Color::Red = c { 1 } else { 0 }\n}\n\npub fn same(c: Color) -> bool {\n    c == Color::Green\n}\n\npub fn build(ok: bool) -> Result<Color, ()> {\n    ok.then_some(Color::Red).ok_or(())\n}\n",
    );
    let text = add_blue(&repo);
    assert_has(&text, "enum `Color` (src/color.rs): variants +1 (Blue), -0");
    assert_has(&text, "Unresolved outside the diff: 5");
    assert_lacks(&text, "Not in your diff");
    assert_lacks(&text, "In your diff");
}

#[test]
fn a_rust_arm_that_names_no_variant_counts_the_match_as_unresolved() {
    let repo = rust_color_repo(
        "use crate::color::Color;\n\npub fn some(o: Option<Color>) -> i32 {\n    match o {\n        Some(Color::Red) => 1,\n        Some(_) => 2,\n        None => 0,\n    }\n}\n\npub fn pair(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, Color::Red) => 1,\n        (_, _) => 2,\n    }\n}\n\npub fn then_name(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, Color::Red) => 1,\n        (_, Color::Green) => 2,\n        (_, _) => 3,\n    }\n}\n\npub fn partial(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, _) => 1,\n        (Color::Green, _) => 2,\n    }\n}\n",
    );
    let text = add_blue(&repo);
    assert_has(&text, "Not in your diff, proven (1):");
    assert_has(&text, "src/paint.rs:28  `partial`");
    assert_lacks(&text, "`some`");
    assert_lacks(&text, "`pair`");
    assert_lacks(&text, "`then_name`");
    assert_has(&text, "Unresolved outside the diff: 3");
}

#[test]
fn a_csharp_switch_arm_that_binds_a_name_counts_the_switch_as_unresolved() {
    let repo = committed_repo(&[
        ("Color.cs", "public enum Color\n{\n    Red,\n    Green\n}\n"),
        (
            "Use.cs",
            "public static class Use\n{\n    public static int Open(Color c)\n    {\n        return c switch\n        {\n            Color.Red => 1,\n            Color.Green => 2,\n        };\n    }\n\n    public static int Bound(Color c)\n    {\n        return c switch\n        {\n            Color.Red => 1,\n            var other => 2,\n        };\n    }\n\n    public static int Typed(Color c)\n    {\n        return c switch\n        {\n            Color.Red => 1,\n            Color other => 2,\n        };\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Color.cs",
        "public enum Color\n{\n    Red,\n    Green,\n    Blue\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (Color.cs): variants +1 (Blue), -0");
    assert_has(&text, "Use.cs:7  `Open`");
    assert_lacks(&text, "`Bound`");
    assert_lacks(&text, "`Typed`");
    assert_has(&text, "Unresolved outside the diff: 2");
}

#[test]
fn a_python_case_that_names_no_variant_counts_the_match_as_unresolved() {
    let repo = committed_repo(&[
        (
            "color.py",
            "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n",
        ),
        (
            "use.py",
            "from color import Color\n\n\ndef pair(a, b):\n    match (a, b):\n        case (Color.RED, Color.RED):\n            return 1\n        case (_, _):\n            return 2\n\n\ndef partial(a, b):\n    match (a, b):\n        case (Color.RED, _):\n            return 1\n        case (Color.GREEN, _):\n            return 2\n",
        ),
    ]);
    write(
        &repo.root,
        "color.py",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (color.py): variants +1 (BLUE), -0");
    assert_has(&text, "use.py:14  `partial`");
    assert_lacks(&text, "`pair`");
    assert_has(&text, "Unresolved outside the diff: 1");
}

#[test]
fn a_typescript_record_keyed_by_the_enum_counts_as_unresolved() {
    let repo = committed_repo(&[
        (
            "src/kind.ts",
            "export enum Kind {\n  Circle,\n  Square,\n}\n",
        ),
        (
            "src/use.ts",
            "import { Kind } from \"./kind\";\n\nexport const names: Record<Kind, string> = {\n  [Kind.Circle]: \"c\",\n  [Kind.Square]: \"s\",\n};\n",
        ),
    ]);
    write(
        &repo.root,
        "src/kind.ts",
        "export enum Kind {\n  Circle,\n  Square,\n  Triangle,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "enum `Kind` (src/kind.ts): variants +1 (Triangle), -0",
    );
    assert_has(&text, "Unresolved outside the diff: 1");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_python_dict_lookup_and_an_if_chain_over_the_enum_count_as_unresolved() {
    let repo = committed_repo(&[
        (
            "color.py",
            "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n",
        ),
        (
            "use.py",
            "from color import Color\n\nNAMES = {Color.RED: \"r\", Color.GREEN: \"g\"}\n\n\ndef lookup(c):\n    return NAMES[c]\n\n\ndef chain(c):\n    if c is Color.RED:\n        return 1\n    elif c is Color.GREEN:\n        return 2\n    raise ValueError(c)\n",
        ),
    ]);
    write(
        &repo.root,
        "color.py",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (color.py): variants +1 (BLUE), -0");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_rust_match_on_self_in_an_impl_of_the_enum_is_listed() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod color;\n"),
        (
            "src/color.rs",
            "pub enum Color {\n    Red,\n    Green,\n}\n\nimpl Color {\n    pub fn name(&self) -> &'static str {\n        match self {\n            Self::Red => \"red\",\n            Self::Green => \"green\",\n        }\n    }\n\n    pub fn fallback(&self) -> i32 {\n        match self {\n            Self::Red => 1,\n            _ => 0,\n        }\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/color.rs",
        "pub enum Color {\n    Red,\n    Green,\n    Blue,\n}\n\nimpl Color {\n    pub fn name(&self) -> &'static str {\n        match self {\n            Self::Red => \"red\",\n            Self::Green => \"green\",\n        }\n    }\n\n    pub fn fallback(&self) -> i32 {\n        match self {\n            Self::Red => 1,\n            _ => 0,\n        }\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "Not in your diff, likely (1):");
    assert_has(&text, "src/color.rs:10  `name`");
    assert_lacks(&text, "`fallback`");
    assert_has(&text, "Unresolved outside the diff: 1");
}

#[test]
fn a_rust_match_on_bare_variants_after_a_glob_import_is_listed() {
    let repo = rust_color_repo(
        "use crate::color::Color;\nuse crate::color::Color::*;\n\npub fn name(c: Color) -> &'static str {\n    match c {\n        Red => \"red\",\n        Green => \"green\",\n    }\n}\n",
    );
    let text = add_blue(&repo);
    assert_has(&text, "Not in your diff, proven (1):");
    assert_has(&text, "src/paint.rs:6  `name`");
}

const JAVA_KIND: &str = "package p;\n\npublic enum Kind {\n    CIRCLE,\n    SQUARE\n}\n";
const JAVA_KIND_AFTER: &str =
    "package p;\n\npublic enum Kind {\n    CIRCLE,\n    SQUARE,\n    TRIANGLE\n}\n";

#[test]
fn a_java_switch_with_no_default_label_is_listed_and_a_default_or_an_equality_test_is_counted_as_unresolved()
 {
    let repo = committed_repo(&[
        ("src/Kind.java", JAVA_KIND),
        (
            "src/Use.java",
            "package p;\n\npublic class Use {\n    static String open(Kind k) {\n        switch (k) {\n            case CIRCLE:\n                return \"c\";\n            case SQUARE:\n                return \"s\";\n        }\n        return \"\";\n    }\n\n    static String closed(Kind k) {\n        switch (k) {\n            case CIRCLE:\n                return \"c\";\n            default:\n                return \"\";\n        }\n    }\n\n    static int compare(Kind k) {\n        return k == Kind.CIRCLE ? 1 : 0;\n    }\n}\n",
        ),
    ]);
    write(&repo.root, "src/Kind.java", JAVA_KIND_AFTER);
    let text = text_of(&repo);
    assert_has(
        &text,
        "enum `Kind` (src/Kind.java): variants +1 (TRIANGLE), -0",
    );
    assert_has(&text, "Not in your diff, proven (1):");
    assert_has(&text, "src/Use.java:6  `open`");
    assert_lacks(&text, "`closed`");
    assert_lacks(&text, "`compare`");
    assert_has(&text, "Unresolved outside the diff: 2");
}

#[test]
fn a_typescript_switch_is_judged_by_its_default_clause() {
    let repo = committed_repo(&[
        ("src/color.ts", "export enum Color {\n  Red,\n  Green,\n}\n"),
        (
            "src/use.ts",
            "import { Color } from \"./color\";\n\nexport function open(c: Color): string {\n  switch (c) {\n    case Color.Red:\n      return \"r\";\n    case Color.Green:\n      return \"g\";\n  }\n  return \"\";\n}\n\nexport function closed(c: Color): string {\n  switch (c) {\n    case Color.Red:\n      return \"r\";\n    default:\n      return \"\";\n  }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/color.ts",
        "export enum Color {\n  Red,\n  Green,\n  Blue,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (src/color.ts): variants +1 (Blue), -0");
    assert_has(&text, "src/use.ts:5  `open`");
    assert_lacks(&text, "`closed`");
    assert_has(&text, "Unresolved outside the diff: 1");
}

#[test]
fn a_python_match_is_judged_by_its_wildcard_case() {
    let repo = committed_repo(&[
        (
            "color.py",
            "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n",
        ),
        (
            "use.py",
            "from color import Color\n\n\ndef open_match(c):\n    match c:\n        case Color.RED:\n            return \"r\"\n        case Color.GREEN:\n            return \"g\"\n\n\ndef closed_match(c):\n    match c:\n        case Color.RED:\n            return \"r\"\n        case _:\n            return \"\"\n",
        ),
    ]);
    write(
        &repo.root,
        "color.py",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Color` (color.py): variants +1 (BLUE), -0");
    assert_has(&text, "use.py:6  `open_match`");
    assert_lacks(&text, "`closed_match`");
    assert_has(&text, "Unresolved outside the diff: 1");
}

fn python_repo() -> Repo {
    committed_repo(&[
        (
            "helpers.py",
            "def helper(a):\n    return a + 1\n\n\ndef keep(a):\n    return a\n",
        ),
        (
            "use.py",
            "from helpers import helper\n\n\ndef run():\n    return helper(1)\n",
        ),
    ])
}

#[test]
fn a_removed_python_function_lists_its_caller() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    let text = text_of(&repo);
    assert_has(&text, "function `helper` (helpers.py): removed");
    assert_has(&text, "use.py:5  `run`");
    assert_lacks(&text, "keep");
}

#[test]
fn a_renamed_function_says_what_it_was_renamed_to() {
    let repo = python_repo();
    write(
        &repo.root,
        "helpers.py",
        "def helper_renamed(a):\n    return a + 1\n\n\ndef keep(a):\n    return a\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `helper` (helpers.py): renamed to `helper_renamed`",
    );
    assert_has(&text, "use.py:5  `run`");
}

#[test]
fn a_body_only_edit_lists_nothing() {
    let repo = python_repo();
    write(
        &repo.root,
        "helpers.py",
        "def helper(a):\n    return a + 2\n\n\ndef keep(a):\n    return a\n",
    );
    let text = text_of(&repo);
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    assert_eq!(
        text,
        format!(
            "check_edit: working tree against {} (HEAD): no symbol changed shape.\n",
            &head[..10]
        )
    );
}

#[test]
fn a_check_leaves_the_main_index_file_and_its_rows_unchanged() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    ensure_fresh_file(&workspace, &repo.db, &conn, "helpers.py").unwrap();
    let stamp = |name: &str| {
        let meta = fs::metadata(repo.db.with_file_name(name));
        meta.map(|m| (m.len(), m.modified().unwrap())).ok()
    };
    let rows = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    };
    let before = (
        stamp("artifact.db"),
        stamp("artifact.db-wal"),
        rows("symbols"),
        rows("files"),
        rows("identifiers"),
    );
    let check = edit_check_op(&workspace, &repo.db, &conn, None).unwrap();
    assert_eq!(check.changes.len(), 1);
    let after = (
        stamp("artifact.db"),
        stamp("artifact.db-wal"),
        rows("symbols"),
        rows("files"),
        rows("identifiers"),
    );
    assert_eq!(before, after);
    let helper_rows: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM symbols WHERE name = 'helper' AND kind != 'import'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(helper_rows, 0);
}

#[test]
fn a_check_removes_its_temporary_folder() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    run_check(&repo, None);
    assert!(!repo.root.join(".code-kb").join("check").exists());
}

#[test]
fn a_check_writes_nothing_outside_the_index_folder() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    run_check(&repo, None);
    let status = git(&repo.root, &["status", "--short"]);
    assert_eq!(status, "M helpers.py");
}

#[test]
fn an_untracked_file_is_one_whole_file_diff() {
    let repo = rust_function_repo();
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32, depth: i32) -> i32 {\n    w * h * depth\n}\n",
    );
    write(
        &repo.root,
        "src/third.rs",
        "use crate::geometry::area;\n\npub fn third() -> i32 {\n    area(1, 2, 3)\n}\n",
    );
    write(
        &repo.root,
        "src/lib.rs",
        "pub mod geometry;\npub mod first;\npub mod second;\npub mod third;\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "src/third.rs:4");
    assert_has(&text, "In your diff (");
    assert_lacks(&text, "src/third.rs:4  `third`");
}

#[test]
fn a_base_argument_compares_the_working_tree_with_that_revision() {
    let repo = python_repo();
    let first = git(&repo.root, &["rev-parse", "HEAD"]);
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    git(&repo.root, &["commit", "-q", "-am", "drop helper"]);
    let against_head = run_check(&repo, None);
    assert!(against_head.changes.is_empty());
    let against_first = run_check(&repo, Some(&first));
    assert_eq!(against_first.source, BaseSource::Given);
    assert_eq!(against_first.rev, first);
    assert_eq!(against_first.changes.len(), 1);
    assert_eq!(against_first.changes[0].change, "removed");
}

#[test]
fn a_session_base_is_named_in_the_header() {
    let repo = python_repo();
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let check = edit_check_with(
        &workspace,
        &repo.db,
        &conn,
        BaseRequest::Session(&head),
        EditLimits::default(),
    )
    .unwrap();
    assert_eq!(check.source, BaseSource::Session);
    assert_has(&format_edit_check(&check), "(session base)");
}

#[test]
fn a_revision_that_does_not_exist_is_an_error_that_names_it() {
    let repo = python_repo();
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let error = edit_check_op(&workspace, &repo.db, &conn, Some("no-such-revision")).unwrap_err();
    assert!(matches!(error, OpError::EditCheck(_)));
    assert_has(&error.to_string(), "no-such-revision");
}

#[test]
fn a_revision_that_looks_like_an_option_is_refused() {
    let repo = python_repo();
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let error = edit_check_op(&workspace, &repo.db, &conn, Some("--output=x")).unwrap_err();
    assert_has(&error.to_string(), "--output=x");
    assert!(!repo.root.join("x").exists());
}

#[test]
fn a_repository_with_no_commits_reports_no_shape_change() {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    git(&root, &["init", "-q", "-b", "main"]);
    write(&root, "a.py", "def f(x):\n    return x\n");
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root.clone());
    scan_workspace(&workspace, &db, true).unwrap();
    let conn = open_read_only(&db).unwrap();
    let check = edit_check_op(&workspace, &db, &conn, None).unwrap();
    assert!(check.changes.is_empty());
    assert_has(&format_edit_check(&check), "no symbol changed shape.");
    assert_has(&format_edit_check(&check), "no commits");
}

#[test]
fn a_detached_head_is_a_base_like_any_commit() {
    let repo = python_repo();
    let head = git(&repo.root, &["rev-parse", "HEAD"]);
    git(&repo.root, &["checkout", "-q", "--detach", &head]);
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    let check = run_check(&repo, None);
    assert_eq!(check.source, BaseSource::Head);
    assert_eq!(check.changes.len(), 1);
}

#[test]
fn a_file_renamed_in_git_keeps_its_symbols_in_place() {
    let repo = python_repo();
    git(&repo.root, &["mv", "helpers.py", "tools.py"]);
    write(
        &repo.root,
        "use.py",
        "from tools import helper\n\n\ndef run():\n    return helper(1)\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "no symbol changed shape.");
}

#[test]
fn a_file_renamed_in_git_still_shows_a_shape_change_made_with_the_rename() {
    let repo = committed_repo(&[
        (
            "helpers.py",
            "def helper(a):\n    return a + 1\n\n\ndef keep(a):\n    return a\n\n\ndef more(a):\n    return a * 3\n\n\ndef most(a):\n    return a * 4\n",
        ),
        (
            "use.py",
            "from helpers import helper\n\n\ndef run():\n    return helper(1)\n",
        ),
    ]);
    git(&repo.root, &["mv", "helpers.py", "tools.py"]);
    write(
        &repo.root,
        "tools.py",
        "def helper(a, b):\n    return a + b\n\n\ndef keep(a):\n    return a\n\n\ndef more(a):\n    return a * 3\n\n\ndef most(a):\n    return a * 4\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `helper` (tools.py): parameters 1 → 2");
    assert_has(&text, "use.py:5  `run`");
}

#[test]
fn paths_with_spaces_are_checked() {
    let repo = committed_repo(&[
        ("my pkg/help ers.py", "def helper(a):\n    return a\n"),
        (
            "my pkg/use it.py",
            "from help_ers import helper\n\n\ndef run():\n    return helper(1)\n",
        ),
    ]);
    write(
        &repo.root,
        "my pkg/help ers.py",
        "def other(a):\n    return a * 2\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `helper` (my pkg/help ers.py): removed");
    assert_has(&text, "my pkg/use it.py:5  `run`");
}

#[test]
fn a_file_that_turns_binary_and_a_new_binary_file_are_not_compared() {
    let repo = committed_repo(&[
        ("a.py", "def f(x):\n    return x\n"),
        ("blob.py", "def g(x):\n    return x\n"),
    ]);
    fs::write(repo.root.join("blob.py"), [0u8, 159, 146, 150, 0, 1]).unwrap();
    fs::write(repo.root.join("image.bin"), [0u8, 1, 2, 3]).unwrap();
    let check = run_check(&repo, None);
    assert!(check.changes.is_empty(), "{:?}", check.changes);
    assert!(check.cuts.is_empty(), "{:?}", check.cuts);
}

#[test]
fn a_rust_method_whose_impl_block_is_in_another_file_keeps_its_owner() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod acc;\npub mod ops;\npub mod run;\n"),
        ("src/acc.rs", "pub struct Acc {\n    pub n: i32,\n}\n"),
        (
            "src/ops.rs",
            "use crate::acc::Acc;\n\nimpl Acc {\n    pub fn add(&mut self, x: i32) {\n        self.n += x;\n    }\n}\n",
        ),
        (
            "src/run.rs",
            "use crate::acc::Acc;\n\npub fn go() {\n    let mut a = Acc { n: 0 };\n    a.add(1);\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/ops.rs",
        "use crate::acc::Acc;\n\nimpl Acc {\n    pub fn add(&mut self, x: i32, y: i32) {\n        self.n += x + y;\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "method `Acc.add` (src/ops.rs): parameters 1 → 2");
    assert_has(&text, "src/run.rs:5  `go`  passes 1 arg; takes 2");
}

#[test]
fn a_changed_return_type_says_the_signature_changed() {
    let repo = committed_repo(&[
        (
            "Calc.java",
            "public class Calc {\n    public int add(int a, int b) {\n        return a + b;\n    }\n}\n",
        ),
        (
            "Main.java",
            "public class Main {\n    public int run(Calc c) {\n        return c.add(1, 2);\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Calc.java",
        "public class Calc {\n    public long add(int a, int b) {\n        return a + b;\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "method `Calc.add` (Calc.java): signature changed");
    assert_has(&text, "Main.java:3  `run`");
}

#[test]
fn a_modifier_added_to_a_signature_is_not_a_shape_change() {
    let repo = committed_repo(&[
        (
            "Calc.java",
            "public class Calc {\n    int add(int a, int b) {\n        return a + b;\n    }\n}\n",
        ),
        (
            "Main.java",
            "public class Main {\n    public int run(Calc c) {\n        return c.add(1, 2);\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Calc.java",
        "public class Calc {\n    public final int add(int a, int b) {\n        return a + b;\n    }\n}\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_struct_literal_that_gains_its_field_on_a_later_line_is_in_the_diff() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod cfg;\npub mod make;\n"),
        (
            "src/cfg.rs",
            "pub struct Cfg {\n    pub a: i32,\n    pub b: i32,\n}\n",
        ),
        (
            "src/make.rs",
            "use crate::cfg::Cfg;\n\npub fn make() -> Cfg {\n    Cfg {\n        a: 1,\n        b: 2,\n    }\n}\n\npub fn other() -> Cfg {\n    Cfg {\n        a: 3,\n        b: 4,\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/cfg.rs",
        "pub struct Cfg {\n    pub a: i32,\n    pub b: i32,\n    pub c: i32,\n}\n",
    );
    write(
        &repo.root,
        "src/make.rs",
        "use crate::cfg::Cfg;\n\npub fn make() -> Cfg {\n    Cfg {\n        a: 1,\n        b: 2,\n        c: 3,\n    }\n}\n\npub fn other() -> Cfg {\n    Cfg {\n        a: 3,\n        b: 4,\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "In your diff (1): src/make.rs:4");
    assert_has(&text, "src/make.rs:12  `other`");
    assert_lacks(&text, "src/make.rs:4  ");
}

#[test]
fn a_struct_that_loses_a_field_says_which() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod cfg;\npub mod make;\n"),
        (
            "src/cfg.rs",
            "pub struct Cfg {\n    pub a: i32,\n    pub b: i32,\n}\n",
        ),
        (
            "src/make.rs",
            "use crate::cfg::Cfg;\n\npub fn make() -> Cfg {\n    Cfg { a: 1, b: 2 }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/cfg.rs",
        "pub struct Cfg {\n    pub a: i32,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Cfg` (src/cfg.rs): fields +0, -1 (b)");
    assert_has(&text, "src/make.rs:4  `make`");
}

#[test]
fn a_deleted_file_removes_its_symbols() {
    let repo = committed_repo(&[
        (
            "gone.py",
            "def first(x):\n    return x\n\n\ndef second(x):\n    return x + 1\n",
        ),
        (
            "use.py",
            "from gone import first, second\n\n\ndef make():\n    return first(1) + second(2)\n",
        ),
    ]);
    fs::remove_file(repo.root.join("gone.py")).unwrap();
    let text = text_of(&repo);
    assert_has(&text, "function `first` (gone.py): removed");
    assert_has(&text, "function `second` (gone.py): removed");
    assert_has(&text, "use.py:5  `make`");
}

#[test]
fn a_removed_method_keeps_its_class_and_lists_the_call() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "class Box:\n    def grow(self, k):\n        return k\n\n    def shrink(self, k):\n        return k * 2\n",
        ),
        (
            "use.py",
            "from shapes import Box\n\n\ndef make():\n    b = Box()\n    return b.grow(2) + b.shrink(1)\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "class Box:\n    def grow(self, k):\n        return k\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "method `Box.shrink` (shapes.py): removed");
    assert_has(&text, "use.py:6  `make`");
    assert_lacks(&text, "grow");
}

#[test]
fn the_tests_that_call_a_changed_function_are_named_with_their_commands() {
    let repo = committed_repo(&[
        ("helpers.py", "def helper(a):\n    return a + 1\n"),
        (
            "tests/test_helpers.py",
            "from helpers import helper\n\n\ndef test_helper():\n    assert helper(1) == 2\n",
        ),
    ]);
    write(
        &repo.root,
        "helpers.py",
        "def helper(a, b):\n    return a + b\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "Tests to run: pytest tests/test_helpers.py::test_helper\n",
    );
}

const CONFIG_APP: &str = "def debug_flag():\n    return True\n\n\ndef make_config():\n    return {\"debug\": debug_flag()}\n\n\nclass App:\n    def __init__(self):\n        self.config = make_config()\n";

fn config_app_repo() -> Repo {
    committed_repo(&[
        ("pytest.ini", "[pytest]\n"),
        ("app.py", CONFIG_APP),
        (
            "tests/conftest.py",
            "import pytest\n\nfrom app import App\n\n\n@pytest.fixture\ndef app():\n    return App()\n",
        ),
        (
            "tests/test_config.py",
            "from app import App\n\n\ndef test_config_debug():\n    assert App().config[\"debug\"]\n",
        ),
        (
            "tests/test_views.py",
            "def test_index(app):\n    assert app.config\n",
        ),
    ])
}

#[test]
fn the_tests_that_reach_the_callers_of_a_changed_function_are_named() {
    let repo = config_app_repo();
    write(
        &repo.root,
        "app.py",
        &CONFIG_APP.replace("def debug_flag():", "def debug_flag(env):"),
    );
    let check = run_check(&repo, None);
    assert!(
        check
            .tests
            .contains(&"pytest tests/test_config.py::test_config_debug".to_string()),
        "{:?}",
        check.tests
    );
    assert!(
        check
            .tests
            .contains(&"pytest tests/test_views.py::test_index".to_string()),
        "{:?}",
        check.tests
    );
    assert_eq!(check.tests_found, 2);
}

#[test]
fn the_tests_that_reach_a_reader_of_a_removed_field_are_named() {
    let repo = committed_repo(&[
        ("pytest.ini", "[pytest]\n"),
        (
            "wrappers.py",
            "class Request:\n    routing_exception: Exception | None = None\n    method: str = \"GET\"\n",
        ),
        (
            "dispatch.py",
            "from wrappers import Request\n\n\ndef dispatch(req: Request):\n    if req.routing_exception is not None:\n        raise req.routing_exception\n    return req.method\n\n\ndef handle():\n    return dispatch(Request())\n",
        ),
        (
            "tests/test_dispatch.py",
            "from dispatch import handle\n\n\ndef test_handle():\n    assert handle() == \"GET\"\n",
        ),
    ]);
    write(
        &repo.root,
        "wrappers.py",
        "class Request:\n    method: str = \"GET\"\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "fields +0, -1 (routing_exception)");
    assert_has(
        &text,
        "Tests to run: pytest tests/test_dispatch.py::test_handle\n",
    );
}

#[test]
fn a_caller_that_no_indexed_call_reaches_names_the_broader_scope_instead_of_tests() {
    let repo = committed_repo(&[
        ("pytest.ini", "[pytest]\n"),
        ("lib.py", "def helper(a):\n    return a\n"),
        (
            "entry.py",
            "from lib import helper\n\n\ndef main():\n    return helper(1)\n",
        ),
    ]);
    write(
        &repo.root,
        "lib.py",
        "def helper(a, b):\n    return a + b\n",
    );
    let check = run_check(&repo, None);
    assert!(check.tests.is_empty(), "{:?}", check.tests);
    assert_has(
        &format_edit_check(&check),
        "No test reaches the changed symbols through the index: the walk stopped at 1 function that no indexed call reaches. Broader test command: `pytest`\n",
    );
}

const ZIG_CALC: &str = "const std = @import(\"std\");\n\nfn target(a: i32) i32 {\n    return a;\n}\n\ntest \"adds two\" {\n    try std.testing.expectEqual(@as(i32, 1), target(1));\n}\n";

#[test]
fn a_test_with_no_run_command_is_listed_by_place_with_the_reason() {
    let repo = committed_repo(&[("src/calc.zig", ZIG_CALC)]);
    write(
        &repo.root,
        "src/calc.zig",
        &ZIG_CALC.replace("fn target(a: i32) i32", "fn target(a: i32, b: i32) i32"),
    );
    let check = run_check(&repo, None);
    let text = format_edit_check(&check);

    assert!(check.tests.is_empty(), "{:?}", check.tests);
    assert_eq!(check.tests_without_command_found, 1);
    assert_has(
        &text,
        "Tests with no run command: `adds two` [src/calc.zig:7]\n",
    );
    assert_has(
        &text,
        "Tests to run may be incomplete: 1 test found has no run command, because no runner the project configures runs it alone. Run the full test suite.\n",
    );
    assert_lacks(&text, "No test reaches");
    assert_has(
        &code_kb_core::formatters::format_edit_check_page(&check, 0, 5),
        "Tests with no run command (1):\n  Page offset 0: rows 1-1 of 1.\n`adds two` [src/calc.zig:7]\n",
    );
}

#[test]
fn a_test_the_walk_reaches_keeps_its_place_when_no_runner_runs_it() {
    let repo = committed_repo(&[
        ("Gemfile", "gem \"rake\"\n"),
        (
            "lib/calc.rb",
            "def helper(a)\n  a\nend\n\ndef wrap\n  helper(1)\nend\n",
        ),
        (
            "spec/calc_spec.rb",
            "require \"calc\"\n\nRSpec.describe \"calc\" do\n  it \"wraps\" do\n    expect(wrap).to eq(1)\n  end\nend\n",
        ),
    ]);
    write(
        &repo.root,
        "lib/calc.rb",
        "def helper(a, b)\n  a + b\nend\n\ndef wrap\n  helper(1)\nend\n",
    );
    let check = run_check(&repo, None);

    assert!(check.tests.is_empty(), "{:?}", check.tests);
    assert_eq!(
        check
            .tests_without_command
            .iter()
            .map(|test| (test.name.as_str(), test.path.as_str(), test.line))
            .collect::<Vec<_>>(),
        [("wraps", "spec/calc_spec.rb", 4)],
        "{check:#?}"
    );
    assert_has(
        &format_edit_check(&check),
        "Tests with no run command: `wraps` [spec/calc_spec.rb:4]\n",
    );
}

#[test]
fn a_test_with_a_command_and_one_without_are_listed_on_their_own_lines() {
    let repo = committed_repo(&[
        ("Gemfile", "gem \"rspec\"\n"),
        ("lib/calc.rb", "def helper(a)\n  a\nend\n"),
        (
            "spec/calc_spec.rb",
            "require \"calc\"\n\nRSpec.describe \"calc\" do\n  it \"helps\" do\n    expect(helper(1)).to eq(1)\n  end\nend\n",
        ),
        (
            "test/calc_test.rb",
            "require \"calc\"\n\nclass CalcTest < Minitest::Test\n  def test_helper\n    assert_equal 1, helper(1)\n  end\nend\n",
        ),
    ]);
    write(
        &repo.root,
        "lib/calc.rb",
        "def helper(a, b)\n  a + b\nend\n",
    );
    let text = text_of(&repo);

    assert_has(
        &text,
        "Tests to run: bundle exec rspec spec/calc_spec.rb:4\nTests with no run command: `test_helper` [test/calc_test.rb:4]\n",
    );
}

#[test]
fn a_test_list_past_the_kept_fifty_states_the_count_and_the_broader_scope() {
    let mut tests = String::from("from app import App\n\n");
    for index in 0..60 {
        tests.push_str(&format!(
            "\ndef test_app_{index}():\n    assert App().config\n\n"
        ));
    }
    let repo = committed_repo(&[
        ("pytest.ini", "[pytest]\n"),
        ("app.py", CONFIG_APP),
        ("tests/test_app.py", &tests),
    ]);
    write(
        &repo.root,
        "app.py",
        &CONFIG_APP.replace("def debug_flag():", "def debug_flag(env):"),
    );
    let check = run_check(&repo, None);
    assert_eq!(check.tests.len(), 50);
    assert_eq!(check.tests_found, 60);
    let text = format_edit_check(&check);
    assert_has(&text, "; … 52 more\n");
    assert_has(
        &text,
        "Tests to run may be incomplete: the check keeps 50 of 60. Broader test command: `pytest`\n",
    );
}

#[test]
fn only_the_thirty_symbols_with_the_most_uses_are_checked_and_the_cut_is_stated() {
    let mut files: Vec<(String, String)> = Vec::new();
    let mut definitions = String::new();
    let mut edited = String::new();
    let mut uses = String::from("from lib import *\n\n\ndef go():\n");
    for index in 0..32 {
        definitions.push_str(&format!("def f{index}(a):\n    return a\n\n\n"));
        edited.push_str(&format!("def f{index}(a, b):\n    return a\n\n\n"));
        for _ in 0..=index {
            uses.push_str(&format!("    f{index}(1)\n"));
        }
    }
    files.push(("lib.py".to_string(), definitions));
    files.push(("use.py".to_string(), uses));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let repo = committed_repo(&borrowed);
    write(&repo.root, "lib.py", &edited);
    let check = run_check(&repo, None);
    assert_eq!(check.changes.len(), 30);
    let names: Vec<&str> = check.changes.iter().map(|c| c.display.as_str()).collect();
    assert!(names.contains(&"f31") && names.contains(&"f2"), "{names:?}");
    assert!(
        !names.contains(&"f0") && !names.contains(&"f1"),
        "{names:?}"
    );
    assert_eq!(
        check.cuts,
        vec!["Checked 30 of 32 changed symbols, those with the most uses of their name first."]
    );
    assert_has(
        &format_edit_check(&check),
        "Cut: Checked 30 of 32 changed symbols",
    );
}

#[test]
fn a_spent_time_budget_stops_the_check_and_says_so() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let limits = EditLimits {
        time: std::time::Duration::ZERO,
        ..EditLimits::default()
    };
    let check = edit_check_with(&workspace, &repo.db, &conn, BaseRequest::Head, limits).unwrap();
    assert!(check.changes.is_empty());
    assert_eq!(check.cuts.len(), 1);
    assert_has(&check.cuts[0], "time budget");
}

#[test]
fn the_diff_hash_changes_with_the_working_tree_and_not_with_a_second_run() {
    let repo = python_repo();
    write(&repo.root, "helpers.py", "def keep(a):\n    return a\n");
    let first = run_check(&repo, None).diff_hash;
    let again = run_check(&repo, None).diff_hash;
    assert_eq!(first, again);
    write(&repo.root, "helpers.py", "def keep(a, b):\n    return a\n");
    let changed = run_check(&repo, None).diff_hash;
    assert_ne!(first, changed);
}

#[test]
fn a_python_dataclass_that_gains_a_field_lists_the_construction_the_diff_did_not_reach() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\n\n\n@dataclass\nclass Point:\n    x: int\n    y: int\n",
        ),
        (
            "use.py",
            "from shapes import Point\n\n\ndef make():\n    return Point(1, 2)\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\n\n\n@dataclass\nclass Point:\n    x: int\n    y: int\n    z: int\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Point` (shapes.py): fields +1 (z), -0");
    assert_has(&text, "use.py:5  `make`");
}

#[test]
fn a_plain_python_class_attribute_is_tracked_without_changing_construction() {
    let repo = committed_repo(&[
        ("shapes.py", "class Point:\n    x = 1\n"),
        (
            "use.py",
            "from shapes import Point\n\n\ndef make():\n    return Point()\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "class Point:\n    x = 1\n    y = 2\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "fields +1 (y), -0");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_go_struct_that_gains_a_field_lists_the_unkeyed_literal_the_diff_did_not_reach() {
    let repo = committed_repo(&[
        (
            "g.go",
            "package g\n\ntype Pt struct {\n\tX int\n\tY int\n}\n",
        ),
        (
            "h.go",
            "package g\n\nfunc mk() Pt {\n\treturn Pt{1, 2}\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "g.go",
        "package g\n\ntype Pt struct {\n\tX int\n\tY int\n\tZ int\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Pt` (g.go): fields +1 (Z), -0");
    assert_has(&text, "h.go:4  `mk`");
}

#[test]
fn a_typescript_class_that_gains_a_field_its_constructor_sets_lists_no_construction() {
    let repo = committed_repo(&[
        (
            "c.ts",
            "export class Box {\n  w: number;\n  constructor(w: number) {\n    this.w = w;\n  }\n}\n",
        ),
        (
            "u.ts",
            "import { Box } from './c';\nexport function mk() { return new Box(1); }\n",
        ),
    ]);
    write(
        &repo.root,
        "c.ts",
        "export class Box {\n  w: number;\n  d: number;\n  constructor(w: number) {\n    this.w = w;\n    this.d = 0;\n  }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Box` (c.ts): fields +1 (d), -0");
    assert_lacks(&text, "u.ts");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_rust_enum_that_loses_a_variant_lists_the_function_with_the_match() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod mode;\npub mod user;\n"),
        (
            "src/mode.rs",
            "pub enum Mode {\n    A,\n    B,\n    C,\n}\n",
        ),
        (
            "src/user.rs",
            "use crate::mode::Mode;\n\npub fn f(m: Mode) -> u8 {\n    match m {\n        Mode::A => 1,\n        Mode::B => 2,\n        Mode::C => 3,\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/mode.rs",
        "pub enum Mode {\n    A,\n    B,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "enum `Mode` (src/mode.rs): variants +0, -1 (C)");
    assert_has(&text, "src/user.rs:7  `f`");
    assert_lacks(&text, "src/user.rs:5");
}

fn rust_kind_repo(user: &str) -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod shapes;\npub mod user;\n"),
        (
            "src/shapes.rs",
            "pub enum Kind {\n    Circle,\n    Square,\n}\n",
        ),
        ("src/user.rs", user),
    ])
}

fn remove_circle(repo: &Repo) -> String {
    write(
        &repo.root,
        "src/shapes.rs",
        "pub enum Kind {\n    Square,\n}\n",
    );
    text_of(repo)
}

#[test]
fn a_function_that_names_only_the_removed_variant_is_listed_at_that_line() {
    let repo = rust_kind_repo(
        "use crate::shapes::Kind;\n\npub fn make() -> Kind {\n    Kind::Circle\n}\n\npub fn square_only(k: Kind) -> bool {\n    matches!(k, Kind::Square)\n}\n",
    );
    let text = remove_circle(&repo);
    assert_has(
        &text,
        "enum `Kind` (src/shapes.rs): variants +0, -1 (Circle)",
    );
    assert_has(&text, "src/user.rs:4  `make`");
    assert_lacks(&text, "`square_only`");
    assert_lacks(&text, "src/user.rs:8");
}

#[test]
fn a_function_that_names_the_removed_variant_after_a_surviving_one_is_listed_at_the_removed_line() {
    let repo = rust_kind_repo(
        "use crate::shapes::Kind;\n\npub fn f(k: Kind) -> i32 {\n    let first = Kind::Square;\n    match k {\n        Kind::Square => 2,\n        Kind::Circle => 1,\n    }\n}\n",
    );
    let text = remove_circle(&repo);
    assert_has(&text, "src/user.rs:7  `f`");
    assert_lacks(&text, "src/user.rs:4");
}

#[test]
fn a_removed_variant_is_found_as_self_and_as_a_bare_imported_name() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod shapes;\npub mod user;\n"),
        (
            "src/shapes.rs",
            "pub enum Kind {\n    Circle,\n    Square,\n}\n\nimpl Kind {\n    pub fn sides(&self) -> u32 {\n        match self {\n            Self::Circle => 0,\n            Self::Square => 4,\n        }\n    }\n\n    pub fn is_square(&self) -> bool {\n        matches!(self, Self::Square)\n    }\n}\n",
        ),
        (
            "src/user.rs",
            "use crate::shapes::Kind;\nuse crate::shapes::Kind::*;\n\npub fn bare(k: Kind) -> u32 {\n    match k {\n        Circle => 1,\n        Square => 2,\n    }\n}\n\npub fn bare_square(k: Kind) -> u32 {\n    match k {\n        Square => 2,\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/shapes.rs",
        "pub enum Kind {\n    Square,\n}\n\nimpl Kind {\n    pub fn sides(&self) -> u32 {\n        match self {\n            Self::Circle => 0,\n            Self::Square => 4,\n        }\n    }\n\n    pub fn is_square(&self) -> bool {\n        matches!(self, Self::Square)\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "variants +0, -1 (Circle)");
    assert_has(&text, "src/shapes.rs:8  `sides`");
    assert_has(&text, "src/user.rs:6  `bare`");
    assert_lacks(&text, "`is_square`");
    assert_lacks(&text, "`bare_square`");
}

#[test]
fn a_function_that_still_names_a_removed_variant_is_not_in_your_diff_when_the_diff_touched_another_line()
 {
    let repo = rust_kind_repo(
        "use crate::shapes::Kind;\n\npub fn f(k: Kind) -> i32 {\n    let base = 10;\n    match k {\n        Kind::Square => base,\n        Kind::Circle => 1,\n    }\n}\n",
    );
    write(
        &repo.root,
        "src/shapes.rs",
        "pub enum Kind {\n    Square,\n}\n",
    );
    write(
        &repo.root,
        "src/user.rs",
        "use crate::shapes::Kind;\n\npub fn f(k: Kind) -> i32 {\n    let base = 20;\n    match k {\n        Kind::Square => base,\n        Kind::Circle => 1,\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "Not in your diff, likely (1):");
    assert_has(&text, "src/user.rs:7  `f`");
    assert_lacks(&text, "In your diff");
}

#[test]
fn a_function_that_dropped_the_removed_variant_in_the_diff_is_not_listed() {
    let repo = rust_kind_repo(
        "use crate::shapes::Kind;\n\npub fn f(k: Kind) -> i32 {\n    match k {\n        Kind::Square => 2,\n        Kind::Circle => 1,\n    }\n}\n",
    );
    write(
        &repo.root,
        "src/shapes.rs",
        "pub enum Kind {\n    Square,\n}\n",
    );
    write(
        &repo.root,
        "src/user.rs",
        "use crate::shapes::Kind;\n\npub fn f(k: Kind) -> i32 {\n    match k {\n        Kind::Square => 2,\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "variants +0, -1 (Circle)");
    assert_lacks(&text, "`f`");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_java_enum_that_loses_a_variant_lists_the_functions_that_name_it() {
    let repo = committed_repo(&[
        ("src/Kind.java", JAVA_KIND),
        (
            "src/Use.java",
            "package p;\n\npublic class Use {\n    static String both(Kind k) {\n        switch (k) {\n            case CIRCLE:\n                return \"c\";\n            case SQUARE:\n                return \"s\";\n        }\n        return \"\";\n    }\n\n    static String square(Kind k) {\n        switch (k) {\n            case SQUARE:\n                return \"s\";\n            default:\n                return \"\";\n        }\n    }\n\n    static Kind make() {\n        return Kind.CIRCLE;\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/Kind.java",
        "package p;\n\npublic enum Kind {\n    SQUARE\n}\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "enum `Kind` (src/Kind.java): variants +0, -1 (CIRCLE)",
    );
    assert_has(&text, "src/Use.java:6  `both`");
    assert_has(&text, "src/Use.java:24  `make`");
    assert_lacks(&text, "`square`");
    assert_lacks(&text, "src/Use.java:8");
    assert_lacks(&text, "src/Use.java:16");
}

#[test]
fn a_variant_added_and_one_removed_in_one_change_lists_both_kinds_of_function() {
    let repo = rust_kind_repo(
        "use crate::shapes::Kind;\n\npub fn uses_circle(k: Kind) -> bool {\n    matches!(k, Kind::Circle)\n}\n\npub fn exhaustive(k: Kind) -> i32 {\n    match k {\n        Kind::Square => 2,\n    }\n}\n\npub fn both(k: Kind) -> i32 {\n    match k {\n        Kind::Circle => 1,\n        Kind::Square => 2,\n    }\n}\n",
    );
    write(
        &repo.root,
        "src/shapes.rs",
        "pub enum Kind {\n    Square,\n    Triangle,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "variants +1 (Triangle), -1 (Circle)");
    assert_has(&text, "src/user.rs:4  `uses_circle`");
    assert_has(&text, "src/user.rs:9  `exhaustive`");
    assert_has(&text, "src/user.rs:15  `both`");
    assert_lacks(&text, "Unresolved outside the diff");
}

#[test]
fn a_file_emptied_in_place_removes_its_symbols() {
    let repo = committed_repo(&[
        ("m.py", "def a(x):\n    return x\n"),
        ("u.py", "from m import a\n\n\ndef f():\n    return a(1)\n"),
    ]);
    write(&repo.root, "m.py", "\n");
    let text = text_of(&repo);
    assert_has(&text, "function `a` (m.py): removed");
    assert_has(&text, "u.py:5  `f`");
}

#[test]
fn without_a_base_the_session_base_of_the_project_is_used_and_named() {
    let repo = rust_function_repo();
    let base = git(&repo.root, &["rev-parse", "HEAD"]);
    record_session_base(&repo.root, "s", &base).unwrap();
    add_depth_parameter(&repo);
    git(&repo.root, &["commit", "-q", "-am", "add depth"]);

    let check = run_check(&repo, None);
    assert_eq!(check.source, BaseSource::Session);
    assert_eq!(check.rev, base[..10]);
    assert_eq!(check.changes.len(), 1);
    assert_has(&format_edit_check(&check), "(session base)");
}

#[test]
fn a_given_base_wins_over_the_session_base() {
    let repo = rust_function_repo();
    let base = git(&repo.root, &["rev-parse", "HEAD"]);
    record_session_base(&repo.root, "s", &base).unwrap();
    add_depth_parameter(&repo);
    git(&repo.root, &["commit", "-q", "-am", "add depth"]);

    let check = run_check(&repo, Some("HEAD"));
    assert_eq!(check.source, BaseSource::Given);
    assert!(check.changes.is_empty());
}

#[test]
fn a_blank_base_means_no_base() {
    let repo = rust_function_repo();
    for blank in ["", "  "] {
        let check = run_check(&repo, Some(blank));
        assert_eq!(check.source, BaseSource::Head);
    }
}

#[test]
fn a_session_base_that_git_no_longer_has_falls_back_to_head() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let gone = "0123456789abcdef0123456789abcdef01234567";

    let check = edit_check_session_op(&workspace, &repo.db, &conn, Some(gone)).unwrap();
    assert_eq!(check.source, BaseSource::Head);
    assert_eq!(check.changes.len(), 1);

    record_session_base(&repo.root, "s", gone).unwrap();
    assert_eq!(run_check(&repo, None).source, BaseSource::Head);
}

#[test]
fn the_head_commit_is_none_before_the_first_commit() {
    let repo = rust_function_repo();
    assert_eq!(
        head_commit(&repo.root),
        Some(git(&repo.root, &["rev-parse", "HEAD"]))
    );
    let plain = safe_tempdir();
    git(plain.path(), &["init", "-q", "-b", "main"]);
    assert_eq!(head_commit(plain.path()), None);
}

fn qualified_call_repo() -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub mod geometry;\npub mod first;\npub mod second;\n",
        ),
        (
            "src/geometry.rs",
            "pub fn area(w: i32, h: i32) -> i32 {\n    w * h\n}\n",
        ),
        (
            "src/first.rs",
            "use crate::geometry;\n\npub fn first() -> i32 {\n    crate::geometry::area(2, 3)\n}\n",
        ),
        (
            "src/second.rs",
            "use crate::geometry;\n\npub fn second() -> i32 {\n    crate::geometry::area(4, 5)\n}\n",
        ),
    ])
}

#[test]
fn the_stop_reason_counts_the_sites_outside_the_diff_and_names_each_change() {
    let repo = qualified_call_repo();
    add_depth_parameter(&repo);
    let reason = format_stop_reason(&run_check(&repo, None)).unwrap();
    assert_eq!(
        reason,
        "Before you finish: 1 sites still use the old shape of `area`.\nsrc/second.rs:4  area: parameters 2 → 3\nEdit them or say why they need no change."
    );
}

#[test]
fn the_stop_reason_is_none_for_no_change_and_for_a_body_only_edit() {
    let repo = python_repo();
    assert_eq!(format_stop_reason(&run_check(&repo, None)), None);
    write(
        &repo.root,
        "helpers.py",
        "def helper(a):\n    return a + 2\n\n\ndef keep(a):\n    return a\n",
    );
    assert_eq!(format_stop_reason(&run_check(&repo, None)), None);
}

#[test]
fn the_stop_reason_is_none_when_the_diff_reached_every_site() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    write(
        &repo.root,
        "src/second.rs",
        "use crate::geometry::area;\n\npub fn second() -> i32 {\n    area(4, 5, 1)\n}\n",
    );
    assert_eq!(format_stop_reason(&run_check(&repo, None)), None);
}

#[test]
fn a_likely_site_never_blocks_a_stop() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    let check = run_check(&repo, None);

    assert!(!check.changes[0].likely.is_empty(), "{check:?}");
    assert_eq!(format_stop_reason(&check), None);
}

#[test]
fn a_signature_change_never_blocks_a_stop() {
    let repo = qualified_call_repo();
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32) -> i64 {\n    (w * h) as i64\n}\n",
    );
    let check = run_check(&repo, None);

    assert!(
        check.changes[0].change.starts_with("signature changed"),
        "{check:?}"
    );
    assert!(!check.changes[0].proven.is_empty(), "{check:?}");
    assert_eq!(format_stop_reason(&check), None);
}

#[test]
fn a_javascript_parameter_count_change_never_blocks_a_stop() {
    let repo = committed_repo(&[
        (
            "lib/geometry.js",
            "function area(w, h) {\n  return w * h;\n}\n\nmodule.exports = { area };\n",
        ),
        (
            "lib/user.js",
            "const { area } = require('./geometry');\n\nfunction user() {\n  return area(1, 2);\n}\n\nmodule.exports = { user };\n",
        ),
    ]);
    write(
        &repo.root,
        "lib/geometry.js",
        "function area(w, h, depth) {\n  return w * h * depth;\n}\n\nmodule.exports = { area };\n",
    );
    let check = run_check(&repo, None);

    assert!(
        check.changes[0].change.starts_with("parameters"),
        "{check:?}"
    );
    assert!(!check.changes[0].proven.is_empty(), "{check:?}");
    assert_eq!(format_stop_reason(&check), None);
}

#[test]
fn a_call_that_an_inherited_overload_takes_is_no_site_of_a_changed_override() {
    let base = "namespace N\n{\n    public abstract class QueryExpression\n    {\n        public abstract bool IsMatch(int root, int t);\n    }\n\n    public class CompositeExpression : QueryExpression\n    {\n        public override bool IsMatch(int root, int t)\n        {\n            return root == t;\n        }\n    }\n}\n";
    let repo = committed_repo(&[
        ("Src/Query.cs", base),
        (
            "Src/Use.cs",
            "namespace N\n{\n    public static class Use\n    {\n        public static bool Run(CompositeExpression c)\n        {\n            return c.IsMatch(1, 2);\n        }\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Src/Query.cs",
        "namespace N\n{\n    public abstract class QueryExpression\n    {\n        public bool IsMatch(int root, int t)\n        {\n            return IsMatch(root, t, 0);\n        }\n\n        public abstract bool IsMatch(int root, int t, int settings);\n    }\n\n    public class CompositeExpression : QueryExpression\n    {\n        public override bool IsMatch(int root, int t, int settings)\n        {\n            return root == t;\n        }\n    }\n}\n",
    );
    let check = run_check(&repo, None);

    let listed: Vec<_> = check
        .changes
        .iter()
        .flat_map(|change| change.proven.iter().chain(&change.likely))
        .filter(|site| site.path == "Src/Use.cs")
        .collect();
    assert!(listed.is_empty(), "{}", format_edit_check(&check));
}

#[test]
fn a_call_that_a_new_static_import_takes_is_no_site_of_the_removed_method() {
    let repo = committed_repo(&[(
        "src/p/Calc.java",
        "package p;\n\npublic class Calc {\n    public int sum(long a, long b) {\n        return toIntExact(a) + toIntExact(b);\n    }\n\n    private static int toIntExact(long x) {\n        return (int) x;\n    }\n}\n",
    )]);
    write(
        &repo.root,
        "src/p/Calc.java",
        "package p;\n\nimport static java.lang.Math.toIntExact;\n\npublic class Calc {\n    public int sum(long a, long b) {\n        return toIntExact(a) + toIntExact(b);\n    }\n}\n",
    );
    let text = text_of(&repo);

    assert_has(&text, "method `Calc.toIntExact` (src/p/Calc.java): removed");
    assert!(!text.contains("Not in your diff"), "{text}");
}

#[test]
fn the_stop_reason_lists_at_most_fifteen_sites_in_path_order() {
    let mut files: Vec<(String, String)> = vec![
        ("Cargo.toml".to_string(), CARGO_TOML.to_string()),
        (
            "src/geometry.rs".to_string(),
            "pub fn area(w: i32, h: i32) -> i32 {\n    w * h\n}\n".to_string(),
        ),
    ];
    let mut lib = String::from("pub mod geometry;\n");
    for index in 0..20 {
        lib.push_str(&format!("pub mod user{index};\n"));
        files.push((
            format!("src/user{index}.rs"),
            format!(
                "use crate::geometry;\n\npub fn user{index}() -> i32 {{\n    crate::geometry::area(1, 2)\n}}\n"
            ),
        ));
    }
    files.push(("src/lib.rs".to_string(), lib));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, content)| (path.as_str(), content.as_str()))
        .collect();
    let repo = committed_repo(&borrowed);
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn area(w: i32, h: i32, depth: i32) -> i32 {\n    w * h * depth\n}\n",
    );
    let check = run_check(&repo, None);
    let reason = format_stop_reason(&check).unwrap();
    let lines: Vec<&str> = reason.lines().collect();
    assert_eq!(
        lines[0],
        "Before you finish: 20 sites still use the old shape of `area`."
    );
    assert_eq!(lines.len(), 17, "{reason}");
    let listed: Vec<&str> = lines[1..16].to_vec();
    let mut sorted = listed.clone();
    sorted.sort_unstable();
    assert_eq!(listed, sorted);
}

const RUST_POINT_WITH_NORM: &str = "pub struct Point {\n    pub x: i32,\n}\n\nimpl Point {\n    pub fn norm(&self, k: i32) -> i32 {\n        self.x * k\n    }\n}\n";
const RUST_POINT_WITHOUT_NORM: &str = "pub struct Point {\n    pub x: i32,\n}\n\nimpl Point {}\n";
const RUST_POINT_WITH_MAGNITUDE: &str = "pub struct Point {\n    pub x: i32,\n}\n\nimpl Point {\n    pub fn magnitude(&self, k: i32) -> i32 {\n        self.x * k\n    }\n}\n";
const RUST_OTHER: &str = "pub struct Other {\n    pub y: i32,\n}\n\nimpl Other {\n    pub fn norm(&self, k: i32) -> i32 {\n        self.y + k\n    }\n}\n";
const RUST_NORM_USERS: &str = "use crate::other::Other;\nuse crate::shapes::Point;\n\npub fn uses_norm(p: &Point) -> i32 {\n    p.norm(3)\n}\n\npub fn uses_other(o: &Other) -> i32 {\n    o.norm(1)\n}\n";

fn rust_method_repo() -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub mod other;\npub mod shapes;\npub mod user;\n",
        ),
        ("src/shapes.rs", RUST_POINT_WITH_NORM),
        ("src/other.rs", RUST_OTHER),
        ("src/user.rs", RUST_NORM_USERS),
    ])
}

#[test]
fn a_removed_rust_method_lists_the_caller_that_reaches_it_through_a_typed_receiver() {
    let repo = rust_method_repo();
    write(&repo.root, "src/shapes.rs", RUST_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "method `Point.norm` (src/shapes.rs): removed");
    assert_has(&text, "src/user.rs:5  `uses_norm`");
}

#[test]
fn a_renamed_rust_method_lists_the_caller_of_the_old_name() {
    let repo = rust_method_repo();
    write(&repo.root, "src/shapes.rs", RUST_POINT_WITH_MAGNITUDE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Point.norm` (src/shapes.rs): renamed to `magnitude`",
    );
    assert_has(&text, "src/user.rs:5  `uses_norm`");
}

#[test]
fn a_removed_rust_method_does_not_list_a_caller_typed_as_another_type_that_keeps_the_name() {
    let repo = rust_method_repo();
    write(&repo.root, "src/shapes.rs", RUST_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "src/user.rs:5  `uses_norm`");
    assert_lacks(&text, "uses_other");
}

const TS_POINT_WITH_NORM: &str = "export class Point {\n  constructor(public x: number) {}\n\n  norm(k: number): number {\n    return this.x * k;\n  }\n}\n";
const TS_POINT_WITHOUT_NORM: &str = "export class Point {\n  constructor(public x: number) {}\n}\n";
const TS_POINT_WITH_MAGNITUDE: &str = "export class Point {\n  constructor(public x: number) {}\n\n  magnitude(k: number): number {\n    return this.x * k;\n  }\n}\n";
const TS_OTHER: &str =
    "export class Other {\n  norm(k: number): number {\n    return k + 1;\n  }\n}\n";
const TS_NORM_USERS: &str = "import { Point } from \"./geom\";\nimport { Other } from \"./other\";\n\nexport function usesNorm(p: Point): number {\n  return p.norm(3);\n}\n\nexport function usesOther(o: Other): number {\n  return o.norm(1);\n}\n";

fn ts_method_repo() -> Repo {
    committed_repo(&[
        ("src/geom.ts", TS_POINT_WITH_NORM),
        ("src/other.ts", TS_OTHER),
        ("src/use.ts", TS_NORM_USERS),
    ])
}

#[test]
fn a_removed_typescript_method_lists_the_caller_that_reaches_it_through_a_typed_receiver() {
    let repo = ts_method_repo();
    write(&repo.root, "src/geom.ts", TS_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "method `Point.norm` (src/geom.ts): removed");
    assert_has(&text, "src/use.ts:5  `usesNorm`");
}

#[test]
fn a_renamed_typescript_method_lists_the_caller_of_the_old_name() {
    let repo = ts_method_repo();
    write(&repo.root, "src/geom.ts", TS_POINT_WITH_MAGNITUDE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Point.norm` (src/geom.ts): renamed to `magnitude`",
    );
    assert_has(&text, "src/use.ts:5  `usesNorm`");
}

#[test]
fn a_removed_typescript_method_does_not_list_a_caller_typed_as_another_class_that_keeps_the_name() {
    let repo = ts_method_repo();
    write(&repo.root, "src/geom.ts", TS_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "src/use.ts:5  `usesNorm`");
    assert_lacks(&text, "usesOther");
}

const JAVA_GEOM_WITH_SCALE: &str = "package p;\n\npublic class Geom {\n    public static int scale(int v) {\n        return v * 2;\n    }\n\n    public static int keep(int v) {\n        return v;\n    }\n}\n";
const JAVA_GEOM_WITHOUT_SCALE: &str = "package p;\n\npublic class Geom {\n    public static int keep(int v) {\n        return v;\n    }\n}\n";
const JAVA_GEOM_WITH_RESCALE: &str = "package p;\n\npublic class Geom {\n    public static int rescale(int v) {\n        return v * 2;\n    }\n\n    public static int keep(int v) {\n        return v;\n    }\n}\n";
const JAVA_STATIC_USERS: &str = "package p;\n\npublic class Use {\n    public static int viaClass() {\n        return Geom.scale(1);\n    }\n}\n";

fn java_static_repo() -> Repo {
    committed_repo(&[
        ("src/Geom.java", JAVA_GEOM_WITH_SCALE),
        ("src/Use.java", JAVA_STATIC_USERS),
    ])
}

#[test]
fn a_removed_java_static_method_lists_the_caller_that_qualifies_it_with_the_class() {
    let repo = java_static_repo();
    write(&repo.root, "src/Geom.java", JAVA_GEOM_WITHOUT_SCALE);
    let text = text_of(&repo);
    assert_has(&text, "method `Geom.scale` (src/Geom.java): removed");
    assert_has(&text, "src/Use.java:5  `viaClass`");
}

#[test]
fn a_renamed_java_static_method_lists_the_caller_of_the_old_name() {
    let repo = java_static_repo();
    write(&repo.root, "src/Geom.java", JAVA_GEOM_WITH_RESCALE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Geom.scale` (src/Geom.java): renamed to `rescale`",
    );
    assert_has(&text, "src/Use.java:5  `viaClass`");
}

const JAVA_POINT_WITH_NORM: &str = "package p;\n\npublic class Point {\n    public int norm(int k) {\n        return k;\n    }\n}\n";
const JAVA_POINT_WITHOUT_NORM: &str = "package p;\n\npublic class Point {\n}\n";
const JAVA_POINT_WITH_MAGNITUDE: &str = "package p;\n\npublic class Point {\n    public int magnitude(int k) {\n        return k;\n    }\n}\n";
const JAVA_OTHER: &str = "package p;\n\npublic class Other {\n    public int norm(int k) {\n        return k + 1;\n    }\n}\n";
const JAVA_INSTANCE_USERS: &str = "package p;\n\npublic class Use {\n    public static int usesNorm(Point p) {\n        return p.norm(3);\n    }\n\n    public static int usesOther(Other o) {\n        return o.norm(1);\n    }\n}\n";

fn java_instance_repo() -> Repo {
    committed_repo(&[
        ("src/Point.java", JAVA_POINT_WITH_NORM),
        ("src/Other.java", JAVA_OTHER),
        ("src/Use.java", JAVA_INSTANCE_USERS),
    ])
}

#[test]
fn a_removed_java_instance_method_lists_the_caller_that_reaches_it_through_a_typed_receiver() {
    let repo = java_instance_repo();
    write(&repo.root, "src/Point.java", JAVA_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "method `Point.norm` (src/Point.java): removed");
    assert_has(&text, "src/Use.java:5  `usesNorm`");
}

#[test]
fn a_renamed_java_instance_method_lists_the_caller_of_the_old_name() {
    let repo = java_instance_repo();
    write(&repo.root, "src/Point.java", JAVA_POINT_WITH_MAGNITUDE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Point.norm` (src/Point.java): renamed to `magnitude`",
    );
    assert_has(&text, "src/Use.java:5  `usesNorm`");
}

#[test]
fn a_removed_java_method_does_not_list_a_caller_typed_as_another_class_that_keeps_the_name() {
    let repo = java_instance_repo();
    write(&repo.root, "src/Point.java", JAVA_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "src/Use.java:5  `usesNorm`");
    assert_lacks(&text, "usesOther");
}

const GO_MOD: &str = "module fx\n\ngo 1.21\n";
const GO_POINT_WITH_NORM: &str = "package geom\n\ntype Point struct {\n\tX int\n}\n\nfunc (p Point) Norm(k int) int {\n\treturn p.X * k\n}\n";
const GO_POINT_WITHOUT_NORM: &str = "package geom\n\ntype Point struct {\n\tX int\n}\n";
const GO_POINT_WITH_MAGNITUDE: &str = "package geom\n\ntype Point struct {\n\tX int\n}\n\nfunc (p Point) Magnitude(k int) int {\n\treturn p.X * k\n}\n";
const GO_OTHER: &str = "package geom\n\ntype Other struct {\n\tY int\n}\n\nfunc (o Other) Norm(k int) int {\n\treturn o.Y + k\n}\n";
const GO_NORM_USER: &str = "package use\n\nimport \"fx/geom\"\n\nfunc UsesNorm() int {\n\tp := geom.Point{X: 1}\n\treturn p.Norm(3)\n}\n";
const GO_NORM_USERS_WITH_OTHER: &str = "package use\n\nimport \"fx/geom\"\n\nfunc UsesNorm() int {\n\tp := geom.Point{X: 1}\n\treturn p.Norm(3)\n}\n\nfunc UsesOther() int {\n\to := geom.Other{Y: 1}\n\treturn o.Norm(1)\n}\n";

fn go_method_repo() -> Repo {
    committed_repo(&[
        ("go.mod", GO_MOD),
        ("geom/geom.go", GO_POINT_WITH_NORM),
        ("use/use.go", GO_NORM_USER),
    ])
}

#[test]
fn a_removed_go_method_lists_the_caller_that_reaches_it_through_a_receiver() {
    let repo = go_method_repo();
    write(&repo.root, "geom/geom.go", GO_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "method `Point.Norm` (geom/geom.go): removed");
    assert_has(&text, "use/use.go:7  `UsesNorm`");
}

#[test]
fn a_renamed_go_method_lists_the_caller_of_the_old_name() {
    let repo = go_method_repo();
    write(&repo.root, "geom/geom.go", GO_POINT_WITH_MAGNITUDE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Point.Norm` (geom/geom.go): renamed to `Magnitude`",
    );
    assert_has(&text, "use/use.go:7  `UsesNorm`");
}

#[test]
fn a_removed_go_method_does_not_list_a_caller_that_another_type_with_the_name_can_take() {
    let repo = committed_repo(&[
        ("go.mod", GO_MOD),
        ("geom/geom.go", GO_POINT_WITH_NORM),
        ("geom/other.go", GO_OTHER),
        ("use/use.go", GO_NORM_USERS_WITH_OTHER),
    ]);
    write(&repo.root, "geom/geom.go", GO_POINT_WITHOUT_NORM);
    let text = text_of(&repo);
    assert_has(&text, "method `Point.Norm` (geom/geom.go): removed");
    assert_lacks(&text, "UsesOther");
    assert_has(&text, "Unresolved outside the diff:");
}

#[test]
fn a_go_call_with_one_argument_is_listed_when_the_function_now_takes_two() {
    let repo = committed_repo(&[
        ("go.mod", GO_MOD),
        (
            "geom/geom.go",
            "package geom\n\nfunc Describe(v int) string {\n\treturn \"\"\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport \"fx/geom\"\n\nfunc UsesDescribe() string {\n\treturn geom.Describe(1)\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "geom/geom.go",
        "package geom\n\nfunc Describe(v, k int) string {\n\treturn \"\"\n}\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `Describe` (geom/geom.go): parameters 1 → 2",
    );
    assert_has(&text, "use/use.go:6  `UsesDescribe`  passes 1 arg; takes 2");
}

#[test]
fn a_call_in_an_untracked_file_that_passes_the_old_count_is_not_in_your_diff() {
    let repo = rust_function_repo();
    add_depth_parameter(&repo);
    write(
        &repo.root,
        "src/extra.rs",
        "use crate::geometry::area;\n\npub fn extra_old() -> i32 {\n    area(1, 2)\n}\n\npub fn extra_new() -> i32 {\n    area(1, 2, 3)\n}\n",
    );
    write(
        &repo.root,
        "src/lib.rs",
        "pub mod geometry;\npub mod first;\npub mod second;\npub mod extra;\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "In your diff (2): src/extra.rs:8, src/first.rs:4");
    assert_has(&text, "src/extra.rs:4  `extra_old`  passes 2 args; takes 3");
}

#[test]
fn an_edited_line_that_keeps_a_call_with_the_old_count_is_not_in_your_diff() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod geometry;\npub mod user;\n"),
        (
            "src/geometry.rs",
            "pub fn spaced(w: i32) -> i32 {\n    w\n}\n",
        ),
        (
            "src/user.rs",
            "use crate::geometry::spaced;\n\npub fn user() -> i32 {\n    spaced(1) + spaced(2)\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/geometry.rs",
        "pub fn spaced(w: i32, k: i32) -> i32 {\n    w + k\n}\n",
    );
    write(
        &repo.root,
        "src/user.rs",
        "use crate::geometry::spaced;\n\npub fn user() -> i32 {\n    spaced(1, 0) + spaced(2)\n}\n",
    );
    let text = text_of(&repo);
    assert_lacks(&text, "In your diff");
    assert_has(&text, "src/user.rs:4  `user`  passes 1 arg; takes 2");
}

#[test]
fn a_renamed_python_function_lists_the_import_of_the_old_name() {
    let repo = committed_repo(&[
        ("pkg/__init__.py", ""),
        ("pkg/geom.py", "def old_name(a):\n    return a + 1\n"),
        (
            "pkg/use.py",
            "from pkg.geom import old_name\n\n\ndef uses_old():\n    return old_name(1)\n",
        ),
    ]);
    write(
        &repo.root,
        "pkg/geom.py",
        "def new_name(a):\n    return a + 1\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `old_name` (pkg/geom.py): renamed to `new_name`",
    );
    assert_has(&text, "pkg/use.py:1");
    assert_has(&text, "pkg/use.py:5  `uses_old`");
}

#[test]
fn an_import_of_a_removed_symbol_that_the_diff_already_dropped_is_not_listed() {
    let repo = committed_repo(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/geom.py",
            "def old_name(a):\n    return a + 1\n\n\ndef keep(a):\n    return a\n",
        ),
        (
            "pkg/use.py",
            "from pkg.geom import old_name, keep\n\n\ndef uses_keep():\n    return keep(1)\n",
        ),
    ]);
    write(&repo.root, "pkg/geom.py", "def keep(a):\n    return a\n");
    write(
        &repo.root,
        "pkg/use.py",
        "from pkg.geom import keep\n\n\ndef uses_keep():\n    return keep(1)\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `old_name` (pkg/geom.py): removed");
    assert_lacks(&text, "pkg/use.py");
}

const JAVA_SHAPE: &str = "package p;\n\npublic interface Shape {\n    int area(int w);\n}\n";
const JAVA_SHAPE_WITH_HEIGHT: &str =
    "package p;\n\npublic interface Shape {\n    int area(int w, int h);\n}\n";
const JAVA_SQ: &str = "package p;\n\npublic class Sq implements Shape {\n    public int area(int w) {\n        return w * w;\n    }\n}\n";
const JAVA_SQ_WITH_HEIGHT: &str = "package p;\n\npublic class Sq implements Shape {\n    public int area(int w, int h) {\n        return w * h;\n    }\n}\n";
const JAVA_SHAPE_USER: &str = "package p;\n\npublic class Use {\n    public static int go(Shape s) {\n        return s.area(2);\n    }\n}\n";

fn java_interface_repo() -> Repo {
    committed_repo(&[
        ("src/Shape.java", JAVA_SHAPE),
        ("src/Sq.java", JAVA_SQ),
        ("src/Use.java", JAVA_SHAPE_USER),
    ])
}

#[test]
fn an_interface_method_that_gains_a_parameter_lists_the_class_that_implements_it() {
    let repo = java_interface_repo();
    write(&repo.root, "src/Shape.java", JAVA_SHAPE_WITH_HEIGHT);
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Shape.area` (src/Shape.java): parameters 1 → 2",
    );
    assert_has(&text, "src/Sq.java:4  `Sq.area`");
    assert_has(&text, "src/Use.java:5  `go`  passes 1 arg; takes 2");
}

#[test]
fn an_implementation_the_diff_updated_is_in_your_diff() {
    let repo = java_interface_repo();
    write(&repo.root, "src/Shape.java", JAVA_SHAPE_WITH_HEIGHT);
    write(&repo.root, "src/Sq.java", JAVA_SQ_WITH_HEIGHT);
    let text = text_of(&repo);
    assert_has(&text, "In your diff (1): src/Sq.java:4");
    assert_lacks(&text, "src/Sq.java:4  `Sq.area`");
}

#[test]
fn a_removed_rust_struct_lists_the_use_line_that_imports_it() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod shapes;\npub mod user;\n"),
        (
            "src/shapes.rs",
            "pub struct Point {\n    pub x: i32,\n}\n\npub struct Keep {\n    pub y: i32,\n}\n",
        ),
        (
            "src/user.rs",
            "use crate::shapes::Point;\n\npub fn origin() -> Point {\n    Point { x: 0 }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/shapes.rs",
        "pub struct Keep {\n    pub y: i32,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Point` (src/shapes.rs): removed");
    assert_has(
        &text,
        "src/user.rs:1  (no enclosing symbol)  imports `Point`",
    );
}

#[test]
fn a_removed_python_function_does_not_list_a_caller_that_imports_the_name_from_another_module() {
    let repo = committed_repo(&[
        ("a.py", "def f(x):\n    return x\n"),
        ("b.py", "def f(x):\n    return x + 1\n"),
        (
            "c.py",
            "from a import f\n\n\ndef from_a():\n    return f(1)\n",
        ),
        (
            "d.py",
            "from b import f\n\n\ndef from_b():\n    return f(1)\n",
        ),
    ]);
    write(&repo.root, "a.py", "def g(x):\n    return x * 3\n");
    let text = text_of(&repo);
    assert_has(&text, "function `f` (a.py): removed");
    assert_has(&text, "c.py:5  `from_a`");
    assert_lacks(&text, "from_b");
    assert_lacks(&text, "d.py");
}

const RUST_CFG: &str =
    "#[derive(Default)]\npub struct Cfg {\n    pub a: i32,\n    pub b: i32,\n}\n";

fn rust_cfg_repo(make: &str) -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod cfg;\npub mod make;\n"),
        ("src/cfg.rs", RUST_CFG),
        ("src/make.rs", make),
    ])
}

#[test]
fn a_rust_literal_that_fills_the_rest_from_a_base_is_not_listed_when_the_struct_gains_a_field() {
    let repo = rust_cfg_repo(
        "use crate::cfg::Cfg;\n\npub fn full() -> Cfg {\n    Cfg { a: 1, b: 2 }\n}\n\npub fn based() -> Cfg {\n    Cfg {\n        a: 1,\n        ..Default::default()\n    }\n}\n",
    );
    write(
        &repo.root,
        "src/cfg.rs",
        "#[derive(Default)]\npub struct Cfg {\n    pub a: i32,\n    pub b: i32,\n    pub c: i32,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Cfg` (src/cfg.rs): fields +1 (c), -0");
    assert_has(&text, "src/make.rs:4  `full`");
    assert_lacks(&text, "based");
}

#[test]
fn a_rust_literal_is_listed_for_a_removed_field_only_when_it_names_the_field() {
    let repo = rust_cfg_repo(
        "use crate::cfg::Cfg;\n\npub fn full() -> Cfg {\n    Cfg { a: 1, b: 2 }\n}\n\npub fn short(b: i32) -> Cfg {\n    Cfg { a: 1, b }\n}\n\npub fn based(base: Cfg) -> Cfg {\n    Cfg { a: 1, ..base }\n}\n",
    );
    write(
        &repo.root,
        "src/cfg.rs",
        "#[derive(Default)]\npub struct Cfg {\n    pub a: i32,\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Cfg` (src/cfg.rs): fields +0, -1 (b)");
    assert_has(&text, "src/make.rs:4  `full`");
    assert_has(&text, "src/make.rs:8  `short`");
    assert_lacks(&text, "based");
}

const GO_PT: &str = "package g\n\ntype Pt struct {\n\tX int\n\tY int\n}\n";
const GO_PT_USES: &str = "package g\n\nfunc keyed() Pt {\n\treturn Pt{X: 1, Y: 2}\n}\n\nfunc unkeyed() Pt {\n\treturn Pt{1, 2}\n}\n\nfunc empty() Pt {\n\treturn Pt{}\n}\n\nfunc many() []Pt {\n\treturn []Pt{{1, 2}}\n}\n\nfunc keyed_many() []Pt {\n\treturn []Pt{{X: 1}}\n}\n";

#[test]
fn a_go_literal_with_keys_or_no_elements_is_not_listed_when_the_struct_gains_a_field() {
    let repo = committed_repo(&[("g.go", GO_PT), ("h.go", GO_PT_USES)]);
    write(
        &repo.root,
        "g.go",
        "package g\n\ntype Pt struct {\n\tX int\n\tY int\n\tZ int\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Pt` (g.go): fields +1 (Z), -0");
    assert_has(&text, "h.go:8  `unkeyed`");
    assert_has(&text, "h.go:16  `many`");
    assert_lacks(&text, "`keyed`");
    assert_lacks(&text, "`empty`");
    assert_lacks(&text, "`keyed_many`");
}

#[test]
fn a_go_literal_is_listed_for_a_removed_field_when_it_is_unkeyed_or_keys_the_field() {
    let repo = committed_repo(&[
        ("g.go", GO_PT),
        (
            "h.go",
            "package g\n\nfunc names() Pt {\n\treturn Pt{X: 1, Y: 2}\n}\n\nfunc unkeyed() Pt {\n\treturn Pt{1, 2}\n}\n\nfunc keeps() Pt {\n\treturn Pt{X: 1}\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "g.go",
        "package g\n\ntype Pt struct {\n\tX int\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Pt` (g.go): fields +0, -1 (Y)");
    assert_has(&text, "h.go:4  `names`");
    assert_has(&text, "h.go:8  `unkeyed`");
    assert_lacks(&text, "`keeps`");
}

const PY_REC: &str = "from dataclasses import dataclass, field\n\n\n@dataclass\nclass Rec:\n    a: int\n    b: int = 0\n";
const PY_REC_USES: &str = "from shapes import Rec\n\n\ndef make():\n    return Rec(1, 2)\n";

#[test]
fn a_python_dataclass_field_with_a_default_lists_no_construction() {
    let repo = committed_repo(&[("shapes.py", PY_REC), ("use.py", PY_REC_USES)]);
    write(
        &repo.root,
        "shapes.py",
        &format!("{PY_REC}    c: int = 0\n    d: list = field(default_factory=list)\n"),
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Rec` (shapes.py): fields +2 (c, d), -0");
    assert_lacks(&text, "use.py");
}

#[test]
fn a_python_dataclass_that_gains_a_field_with_and_a_field_without_a_default_lists_the_construction()
{
    let repo = committed_repo(&[("shapes.py", PY_REC), ("use.py", PY_REC_USES)]);
    write(
        &repo.root,
        "shapes.py",
        &format!("{PY_REC}    c: int = 0\n    d: int\n"),
    );
    assert_has(&text_of(&repo), "use.py:5  `make`");
}

#[test]
fn a_python_dataclass_that_loses_a_field_lists_every_construction() {
    let repo = committed_repo(&[("shapes.py", PY_REC), ("use.py", PY_REC_USES)]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\n\n\n@dataclass\nclass Rec:\n    a: int\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Rec` (shapes.py): fields +0, -1 (b)");
    assert_has(&text, "use.py:5  `make`");
}

const JAVA_RECORD: &str = "public record Point(int x, int y) {}\n";
const JAVA_RECORD_USE: &str =
    "public class Use {\n    public Point make() {\n        return new Point(1, 2);\n    }\n}\n";

#[test]
fn a_java_record_that_gains_or_loses_a_component_lists_the_construction() {
    let repo = committed_repo(&[("Point.java", JAVA_RECORD), ("Use.java", JAVA_RECORD_USE)]);
    write(
        &repo.root,
        "Point.java",
        "public record Point(int x, int y, int z) {}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Point` (Point.java): fields +1 (z), -0");
    assert_has(&text, "Use.java:3  `make`");

    write(&repo.root, "Point.java", "public record Point(int x) {}\n");
    let text = text_of(&repo);
    assert_has(&text, "class `Point` (Point.java): fields +0, -1 (y)");
    assert_has(&text, "Use.java:3  `make`");
}

const JAVA_CLASS: &str = "public class Point {\n    private int x;\n\n    public Point(int x) {\n        this.x = x;\n    }\n}\n";
const JAVA_CLASS_USE: &str =
    "public class Use {\n    public Point make() {\n        return new Point(1);\n    }\n}\n";

#[test]
fn a_java_class_that_gains_a_field_its_constructor_sets_lists_no_construction() {
    let repo = committed_repo(&[("Point.java", JAVA_CLASS), ("Use.java", JAVA_CLASS_USE)]);
    write(
        &repo.root,
        "Point.java",
        "public class Point {\n    private int x;\n    private int z;\n\n    public Point(int x) {\n        this.x = x;\n        this.z = 0;\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Point` (Point.java): fields +1 (z), -0");
    assert_lacks(&text, "Use.java");
}

#[test]
fn a_typescript_class_that_loses_a_field_lists_no_construction() {
    let repo = committed_repo(&[
        (
            "c.ts",
            "export class Box {\n  w: number;\n  h: number;\n  constructor(w: number) {\n    this.w = w;\n    this.h = 0;\n  }\n}\n",
        ),
        (
            "u.ts",
            "import { Box } from './c';\nexport function mk() { return new Box(1); }\n",
        ),
    ]);
    write(
        &repo.root,
        "c.ts",
        "export class Box {\n  w: number;\n  constructor(w: number) {\n    this.w = w;\n  }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "class `Box` (c.ts): fields +0, -1 (h)");
    assert_lacks(&text, "u.ts");
}

const RUST_PLAIN: &str =
    "pub fn plain(a: i32) -> i32 {\n    a\n}\n\npub fn caller() -> i32 {\n    plain(1)\n}\n";

#[test]
fn a_rust_parameter_rename_is_no_shape_change() {
    let repo = committed_repo(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", RUST_PLAIN)]);
    write(
        &repo.root,
        "src/lib.rs",
        &RUST_PLAIN
            .replace("a: i32", "b: i32")
            .replace("    a\n", "    b\n"),
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_rust_parameter_type_change_says_the_signature_changed_and_lists_the_caller() {
    let repo = committed_repo(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", RUST_PLAIN)]);
    write(
        &repo.root,
        "src/lib.rs",
        &RUST_PLAIN
            .replace("a: i32", "a: i64")
            .replace("    a\n", "    a as i32\n"),
    );
    let text = text_of(&repo);
    assert_has(&text, "function `plain` (src/lib.rs): signature changed");
    assert_has(&text, "src/lib.rs:6  `caller`");
}

#[test]
fn a_rust_binding_mut_added_to_a_parameter_is_no_shape_change() {
    let repo = committed_repo(&[("Cargo.toml", CARGO_TOML), ("src/lib.rs", RUST_PLAIN)]);
    write(
        &repo.root,
        "src/lib.rs",
        &RUST_PLAIN.replace("a: i32", "mut a: i32"),
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_rust_reference_made_mutable_says_the_signature_changed() {
    let repo = committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub fn fill(v: &Vec<u8>) {}\n\npub fn caller(v: &Vec<u8>) {\n    fill(v)\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/lib.rs",
        "pub fn fill(v: &mut Vec<u8>) {}\n\npub fn caller(v: &Vec<u8>) {\n    fill(v)\n}\n",
    );
    assert_has(
        &text_of(&repo),
        "function `fill` (src/lib.rs): signature changed",
    );
}

#[test]
fn a_go_group_of_parameters_written_as_two_is_no_shape_change() {
    let repo = committed_repo(&[
        ("go.mod", GO_MOD),
        (
            "geom/geom.go",
            "package geom\n\nfunc Sum(a, b int) int {\n\treturn a + b\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport \"fx/geom\"\n\nfunc Total() int {\n\treturn geom.Sum(1, 2)\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "geom/geom.go",
        "package geom\n\nfunc Sum(a int, b int) int {\n\treturn a + b\n}\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

fn python_scale_repo() -> Repo {
    committed_repo(&[
        (
            "m.py",
            "def scale(value, factor=1):\n    return value * factor\n",
        ),
        (
            "use.py",
            "from m import scale\n\n\ndef positional():\n    return scale(1, 2)\n\n\ndef keyword():\n    return scale(1, factor=2)\n",
        ),
    ])
}

#[test]
fn a_python_default_value_change_is_no_shape_change() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value, factor=2):\n    return value * factor\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_python_annotation_added_to_a_function_is_no_shape_change() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value: int, factor: int = 1) -> int:\n    return value * factor\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_python_parameter_rename_lists_the_caller_that_names_the_old_parameter() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value, ratio=1):\n    return value * ratio\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `scale` (m.py): parameter renamed: factor → ratio",
    );
    assert_has(&text, "use.py:9  `keyword`");
    assert_lacks(&text, "`positional`");
}

#[test]
fn a_python_swap_of_two_parameter_names_says_the_signature_changed() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(factor, value=1):\n    return value * factor\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `scale` (m.py): signature changed");
    assert_has(&text, "use.py:5  `positional`");
}

#[test]
fn a_typescript_parameter_rename_is_no_shape_change() {
    let repo = committed_repo(&[
        (
            "m.ts",
            "export function scale(value: number, factor: number): number {\n  return value * factor;\n}\n",
        ),
        (
            "u.ts",
            "import { scale } from './m';\nexport function run() { return scale(1, 2); }\n",
        ),
    ]);
    write(
        &repo.root,
        "m.ts",
        "export function scale(value: number, ratio: number): number {\n  return value * ratio;\n}\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_java_parameter_type_change_says_the_signature_changed() {
    let repo = committed_repo(&[
        (
            "Calc.java",
            "public class Calc {\n    public int add(int a, int b) {\n        return a + b;\n    }\n}\n",
        ),
        (
            "Main.java",
            "public class Main {\n    public int run(Calc c) {\n        return c.add(1, 2);\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Calc.java",
        "public class Calc {\n    public int add(long a, int b) {\n        return 0;\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "method `Calc.add` (Calc.java): signature changed");
    assert_has(&text, "Main.java:3  `run`");
}

#[test]
fn a_python_keyword_call_that_already_uses_the_new_name_is_not_listed() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value, ratio=1):\n    return value * ratio\n",
    );
    write(
        &repo.root,
        "use.py",
        "from m import scale\n\n\ndef positional():\n    return scale(1, 2)\n\n\ndef keyword():\n    return scale(1, ratio=2)\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "parameter renamed: factor → ratio");
    assert_lacks(&text, "Not in your diff");
}

#[test]
fn a_python_keyword_argument_on_a_later_line_of_the_call_is_listed() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value, ratio=1):\n    return value * ratio\n",
    );
    write(
        &repo.root,
        "use.py",
        "from m import scale\n\n\ndef spread():\n    return scale(\n        1,\n        factor=2,\n    )\n",
    );
    assert_has(
        &text_of(&repo),
        "use.py:5  `spread`  names parameter `factor`",
    );
}

#[test]
fn a_python_constructor_parameter_rename_lists_the_call_that_names_it() {
    let repo = committed_repo(&[
        (
            "m.py",
            "class Box:\n    def __init__(self, size, pad=0):\n        self.size = size\n",
        ),
        (
            "use.py",
            "from m import Box\n\n\ndef make():\n    return Box(1, pad=2)\n\n\ndef plain():\n    return Box(1, 2)\n",
        ),
    ]);
    write(
        &repo.root,
        "m.py",
        "class Box:\n    def __init__(self, size, margin=0):\n        self.size = size\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "constructor `Box.__init__` (m.py): parameter renamed: pad → margin",
    );
    assert_has(&text, "use.py:5  `make`  names parameter `pad`");
    assert_lacks(&text, "`plain`");
}

#[test]
fn a_kotlin_parameter_rename_lists_the_call_that_names_the_old_parameter() {
    let repo = committed_repo(&[
        (
            "m.kt",
            "fun scale(value: Int, factor: Int = 1): Int = value * factor\n",
        ),
        (
            "u.kt",
            "fun positional(): Int = scale(1, 2)\n\nfun named(): Int = scale(1, factor = 2)\n",
        ),
    ]);
    write(
        &repo.root,
        "m.kt",
        "fun scale(value: Int, ratio: Int = 1): Int = value * ratio\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `scale` (m.kt): parameter renamed: factor → ratio",
    );
    assert_has(&text, "u.kt:3  `named`  names parameter `factor`");
    assert_lacks(&text, "`positional`");
}

#[test]
fn a_csharp_parameter_rename_lists_the_call_that_names_the_old_parameter() {
    let repo = committed_repo(&[
        (
            "M.cs",
            "public class M {\n    public static int Scale(int value, int factor) { return value * factor; }\n}\n",
        ),
        (
            "U.cs",
            "public class U {\n    public int Positional() { return M.Scale(1, 2); }\n    public int Named() { return M.Scale(1, factor: 2); }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "M.cs",
        "public class M {\n    public static int Scale(int value, int ratio) { return value * ratio; }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `M.Scale` (M.cs): parameter renamed: factor → ratio",
    );
    assert_has(&text, "U.cs:3  `Named`  names parameter `factor`");
    assert_lacks(&text, "`Positional`");
}

#[test]
fn a_csharp_call_whose_argument_types_select_another_overload_is_not_listed() {
    let repo = committed_repo(&[
        (
            "JValue.cs",
            "public class JValue {\n    public bool Same(object other) { return false; }\n    public bool Same(JValue other) { return true; }\n}\n",
        ),
        (
            "Use.cs",
            "public class Use {\n    public bool Typed(JValue a, JValue b) { return a.Same(b); }\n    public bool Loose(JValue a, object b) { return a.Same(b); }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "JValue.cs",
        "public class JValue {\n    public bool Same(object? other) { return false; }\n    public bool Same(JValue other) { return true; }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "Use.cs:3  `Loose`");
    assert_lacks(&text, "`Typed`");
}

#[test]
fn a_csharp_call_is_not_bound_to_an_overload_with_a_different_namespace() {
    for (left, right) in [("A.Token", "B.Token"), ("Custom.String", "string")] {
        let repo = committed_repo(&[
            (
                "Types.cs",
                "namespace A { public class Token {} }\nnamespace B { public class Token {} }\nnamespace Custom { public class String {} }\n",
            ),
            (
                "Picker.cs",
                &format!(
                    "public class Picker {{\n    public void Pick({left} value) {{}}\n    public void Pick({right} value) {{}}\n}}\n"
                ),
            ),
            (
                "Use.cs",
                &format!(
                    "public class Use {{\n    public void Run(Picker picker, {left} item) {{ picker.Pick(item); }}\n}}\n"
                ),
            ),
        ]);
        write(
            &repo.root,
            "Picker.cs",
            &format!(
                "public class Picker {{\n    public void Pick({left} value, string suffix) {{}}\n    public void Pick({right} value) {{}}\n}}\n"
            ),
        );
        assert_has(&text_of(&repo), "Use.cs:2  `Run`");
    }
}

#[test]
fn a_csharp_call_that_still_fits_the_changed_overload_is_not_listed() {
    let repo = committed_repo(&[
        (
            "Picker.cs",
            "public class Picker {\n    public void Pick(string value, int count = 0) {}\n    public void Pick(int value) {}\n}\n",
        ),
        (
            "Use.cs",
            "public class Use {\n    public void Run(Picker picker) { picker.Pick(\"value\", 2); }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Picker.cs",
        "public class Picker {\n    public void Pick(string value, int count) {}\n    public void Pick(int value) {}\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "parameters");
    assert_lacks(&text, "`Run`");
}

#[test]
fn a_swift_parameter_rename_lists_the_call_that_names_the_old_parameter() {
    let repo = committed_repo(&[
        (
            "m.swift",
            "func scale(value: Int, factor: Int) -> Int { return value * factor }\n",
        ),
        (
            "u.swift",
            "func named() -> Int { return scale(value: 1, factor: 2) }\n",
        ),
    ]);
    write(
        &repo.root,
        "m.swift",
        "func scale(value: Int, ratio: Int) -> Int { return value * ratio }\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `scale` (m.swift): parameter renamed: factor → ratio",
    );
    assert_has(&text, "u.swift:1  `named`  names parameter `factor`");
}

#[test]
fn a_go_method_with_a_renamed_receiver_and_parameters_is_no_shape_change() {
    let repo = committed_repo(&[
        ("go.mod", GO_MOD),
        (
            "geom/geom.go",
            "package geom\n\ntype P struct{}\n\nfunc (p *P) Add(a, b int) int {\n\treturn a + b\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport \"fx/geom\"\n\nfunc Total() int {\n\tp := &geom.P{}\n\treturn p.Add(1, 2)\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "geom/geom.go",
        "package geom\n\ntype P struct{}\n\nfunc (q *P) Add(x, y int) int {\n\treturn x + y\n}\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_java_final_parameter_and_a_renamed_parameter_are_no_shape_change() {
    let repo = committed_repo(&[
        (
            "Calc.java",
            "public class Calc {\n    public int add(int a, int b) {\n        return a + b;\n    }\n}\n",
        ),
        (
            "Main.java",
            "public class Main {\n    public int run(Calc c) {\n        return c.add(1, 2);\n    }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "Calc.java",
        "public class Calc {\n    public int add(final int x, int y) {\n        return x + y;\n    }\n}\n",
    );
    assert_has(&text_of(&repo), "no symbol changed shape.");
}

#[test]
fn a_python_keyword_only_marker_added_to_the_parameters_says_the_signature_changed() {
    let repo = python_scale_repo();
    write(
        &repo.root,
        "m.py",
        "def scale(value, *, factor=1):\n    return value * factor\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "function `scale` (m.py): signature changed");
    assert_has(&text, "use.py:5  `positional`");
}

fn edited_text(files: &[(&str, &str)], path: &str, from: &str, to: &str) -> String {
    let repo = committed_repo(files);
    let content = fs::read_to_string(repo.root.join(path)).unwrap();
    assert!(content.contains(from), "missing `{from}` in {path}");
    write(&repo.root, path, &content.replacen(from, to, 1));
    text_of(&repo)
}

const GO_GEOM: &str = "package geom\n\ntype Point struct {\n\tX int\n\tY int\n}\n";
const GO_QUALIFIED_USES: &str = "package use\n\nimport \"fx/geom\"\n\nfunc slice() []geom.Point {\n\treturn []geom.Point{{1, 2}}\n}\n\nfunc pointers() []*geom.Point {\n\treturn []*geom.Point{{1, 2}}\n}\n\nfunc table() map[string]geom.Point {\n\treturn map[string]geom.Point{\"a\": {1, 2}}\n}\n\nfunc keyed_slice() []geom.Point {\n\treturn []geom.Point{{X: 1, Y: 2}}\n}\n\nfunc keyed_pointers() []*geom.Point {\n\treturn []*geom.Point{{X: 1}}\n}\n\nfunc keyed_table() map[string]geom.Point {\n\treturn map[string]geom.Point{\"a\": {X: 1}}\n}\n";

#[test]
fn a_go_collection_literal_of_a_package_qualified_type_is_judged_by_its_elements() {
    let text = edited_text(
        &[
            ("go.mod", GO_MOD),
            ("geom/geom.go", GO_GEOM),
            ("use/use.go", GO_QUALIFIED_USES),
        ],
        "geom/geom.go",
        "\tY int\n",
        "\tY int\n\tZ int\n",
    );
    assert_has(&text, "struct `Point` (geom/geom.go): fields +1 (Z), -0");
    for listed in ["slice", "pointers", "table"] {
        assert_has(&text, &format!("`{listed}`"));
    }
    for kept in ["keyed_slice", "keyed_pointers", "keyed_table"] {
        assert_lacks(&text, &format!("`{kept}`"));
    }
}

const JAVA_OVERLOADED: &str = "public class B {\n    public int norm() {\n        return 3;\n    }\n    public static int norm(int x) {\n        return x;\n    }\n}\n";
const JAVA_OVERLOAD_USES: &str = "public class U {\n    public static int useB(B b) {\n        return b.norm();\n    }\n    public static int useStatic() {\n        return B.norm(3);\n    }\n}\n";

#[test]
fn a_removed_overload_lists_only_the_call_that_no_remaining_overload_takes() {
    let files = [("B.java", JAVA_OVERLOADED), ("U.java", JAVA_OVERLOAD_USES)];
    let static_gone = edited_text(
        &files,
        "B.java",
        "    public static int norm(int x) {\n        return x;\n    }\n",
        "",
    );
    assert_has(&static_gone, "method `B.norm` (B.java): removed");
    assert_has(
        &static_gone,
        "U.java:6  `useStatic`  passes 1 arg; no remaining overload takes it",
    );
    assert_lacks(&static_gone, "`useB`");
    let instance_gone = edited_text(
        &files,
        "B.java",
        "    public int norm() {\n        return 3;\n    }\n",
        "",
    );
    assert_has(
        &instance_gone,
        "U.java:3  `useB`  passes 0 args; no remaining overload takes it",
    );
    assert_lacks(&instance_gone, "`useStatic`");
}

#[test]
fn a_renamed_overload_lists_only_the_call_that_no_remaining_overload_takes() {
    let text = edited_text(
        &[("B.java", JAVA_OVERLOADED), ("U.java", JAVA_OVERLOAD_USES)],
        "B.java",
        "public static int norm(int x)",
        "public static int magnitude(int x)",
    );
    assert_has(&text, "method `B.norm` (B.java): renamed to `magnitude`");
    assert_has(
        &text,
        "U.java:6  `useStatic`  passes 1 arg; no remaining overload takes it",
    );
    assert_lacks(&text, "`useB`");
}

const KOTLIN_OVERLOADED: &str = "fun f(a: Int): Int = a\nfun f(a: Int, b: Int): Int = a\n";
const KOTLIN_OVERLOAD_USES: &str =
    "fun spread(xs: IntArray): Int = f(*xs)\nfun two(): Int = f(1, 2)\nfun one(): Int = f(1)\n";

#[test]
fn a_call_with_an_unknown_count_is_unresolved_when_overloads_remain() {
    let text = edited_text(
        &[("M.kt", KOTLIN_OVERLOADED), ("U.kt", KOTLIN_OVERLOAD_USES)],
        "M.kt",
        "fun f(a: Int, b: Int): Int = a\n",
        "",
    );
    assert_has(&text, "function `f` (M.kt): removed");
    assert_has(
        &text,
        "U.kt:2  `two`  passes 2 args; no remaining overload takes it",
    );
    assert_has(&text, "Unresolved outside the diff: 1");
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`spread`");
}

#[test]
fn an_import_of_a_name_is_not_listed_when_an_overload_of_it_remains() {
    let text = edited_text(
        &[
            ("pkg/__init__.py", ""),
            (
                "pkg/m.py",
                "from typing import overload\n\n\n@overload\ndef f(a: int) -> int: ...\n@overload\ndef f(a: int, b: int) -> int: ...\ndef f(a, b=0):\n    return a\n",
            ),
            (
                "pkg/u.py",
                "from pkg.m import f\n\n\ndef g(xs):\n    return f(*xs)\n\n\ndef h():\n    return f(1, 2)\n",
            ),
        ],
        "pkg/m.py",
        "@overload\ndef f(a: int, b: int) -> int: ...\n",
        "",
    );
    assert_has(&text, "function `f` (pkg/m.py): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "imports");
    assert_lacks(&text, "`h`");
    assert_lacks(&text, "`g`");
}

const JAVA_SAME_ARITY: &str = "public class B {\n    public static int norm(int a) {\n        return a;\n    }\n    public int norm(String s) {\n        return 1;\n    }\n}\n";
const JAVA_SAME_ARITY_USES: &str = "public class U {\n    public static int useInt() {\n        return B.norm(3);\n    }\n    public static int useString(B b) {\n        return b.norm(\"x\");\n    }\n}\n";

#[test]
fn a_call_that_the_removed_and_a_remaining_overload_both_take_is_counted_as_unresolved() {
    let text = edited_text(
        &[
            ("B.java", JAVA_SAME_ARITY),
            ("U.java", JAVA_SAME_ARITY_USES),
        ],
        "B.java",
        "    public int norm(String s) {\n        return 1;\n    }\n",
        "",
    );
    assert_has(&text, "method `B.norm` (B.java): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "`useInt`");
    assert_lacks(&text, "`useString`");
}

const JAVA_CONSTRUCTORS: &str = "public class P {\n    public P(int a) {\n    }\n    public P(int a, int b) {\n    }\n    public P(String s) {\n    }\n}\n";
const JAVA_CONSTRUCTOR_USES: &str = "public class PU {\n    public static P one() {\n        return new P(1);\n    }\n    public static P two() {\n        return new P(1, 2);\n    }\n    public static P text() {\n        return new P(\"s\");\n    }\n}\n";

#[test]
fn a_removed_java_constructor_lists_only_the_call_that_no_remaining_constructor_takes() {
    let text = edited_text(
        &[
            ("P.java", JAVA_CONSTRUCTORS),
            ("PU.java", JAVA_CONSTRUCTOR_USES),
        ],
        "P.java",
        "    public P(int a, int b) {\n    }\n",
        "",
    );
    assert_has(&text, "constructor `P.P` (P.java): removed");
    assert_has(
        &text,
        "PU.java:6  `two`  passes 2 args; no remaining overload takes it",
    );
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`text`");
    assert_lacks(&text, "Unresolved outside the diff");
}

#[test]
fn a_removed_java_constructor_counts_a_call_that_a_remaining_one_also_takes_as_unresolved() {
    let text = edited_text(
        &[
            ("P.java", JAVA_CONSTRUCTORS),
            ("PU.java", JAVA_CONSTRUCTOR_USES),
        ],
        "P.java",
        "    public P(int a) {\n    }\n",
        "",
    );
    assert_has(&text, "constructor `P.P` (P.java): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`text`");
    assert_lacks(&text, "`two`");
}

#[test]
fn removing_the_only_java_constructor_lists_every_call() {
    let text = edited_text(
        &[
            (
                "C.java",
                "public class C {\n    public C(int a) {\n    }\n}\n",
            ),
            (
                "CU.java",
                "public class CU {\n    public static C make() {\n        return new C(1);\n    }\n}\n",
            ),
        ],
        "C.java",
        "    public C(int a) {\n    }\n",
        "",
    );
    assert_has(&text, "constructor `C.C` (C.java): removed");
    assert_has(&text, "CU.java:3  `make`");
}

#[test]
fn a_removed_constructor_of_a_java_record_counts_its_calls_as_unresolved() {
    let text = edited_text(
        &[
            (
                "Point.java",
                "public record Point(int x, int y) {\n    public Point(int x) {\n        this(x, 0);\n    }\n}\n",
            ),
            ("Use.java", JAVA_RECORD_USES),
        ],
        "Point.java",
        "    public Point(int x) {\n        this(x, 0);\n    }\n",
        "",
    );
    assert_has(&text, "constructor `Point.Point` (Point.java): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`two`");
}

const KOTLIN_CONSTRUCTOR_USES: &str = "fun one() = P(1)\nfun two() = P(1, 2)\n";

#[test]
fn a_removed_kotlin_constructor_drops_the_call_a_remaining_one_takes_and_counts_the_other() {
    let text = edited_text(
        &[
            (
                "P.kt",
                "class P {\n    constructor(a: Int)\n    constructor(a: Int, b: Int)\n}\n",
            ),
            ("U.kt", KOTLIN_CONSTRUCTOR_USES),
        ],
        "P.kt",
        "    constructor(a: Int, b: Int)\n",
        "",
    );
    assert_has(&text, "constructor `P.P` (P.kt): removed");
    assert_has(&text, "Unresolved outside the diff: 1");
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`two`");
}

#[test]
fn a_removed_kotlin_secondary_constructor_counts_its_calls_as_unresolved_beside_a_primary_one() {
    let text = edited_text(
        &[
            (
                "P.kt",
                "class P(val a: Int) {\n    constructor(a: Int, b: Int) : this(a)\n}\n",
            ),
            ("U.kt", KOTLIN_CONSTRUCTOR_USES),
        ],
        "P.kt",
        "    constructor(a: Int, b: Int) : this(a)\n",
        "",
    );
    assert_has(&text, "constructor `P.P` (P.kt): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "`one`");
    assert_lacks(&text, "`two`");
}

#[test]
fn a_removed_function_that_nothing_calls_has_no_sites_and_no_cut() {
    let text = edited_text(
        &[(
            "a.py",
            "def f(x):\n    return x\n\n\ndef g():\n    return 2\n",
        )],
        "a.py",
        "def g():\n    return 2\n",
        "",
    );
    assert_has(&text, "function `g` (a.py): removed");
    assert_lacks(&text, "Cut:");
    assert_lacks(&text, "Not in your diff");
    assert_lacks(&text, "Unresolved outside the diff");
}

type NamedCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    &'static str,
    &'static str,
    &'static str,
    Vec<&'static str>,
    Vec<&'static str>,
);

fn nested_named_argument_cases() -> Vec<NamedCase> {
    vec![
        (
            "python",
            vec![
                ("pkg/__init__.py", ""),
                (
                    "pkg/m.py",
                    "def f(old, b):\n    return old + b\n\n\ndef g(old):\n    return old\n",
                ),
                (
                    "pkg/u.py",
                    "from pkg.m import f, g\n\n\ndef other():\n    return f(1, g(old=2))\n\n\ndef same():\n    return f(f(old=1, b=2), 2)\n\n\ndef spread():\n    return f(\n        1,\n        b=g(\n            old=2,\n        ),\n    )\n",
                ),
            ],
            "pkg/m.py",
            "def f(old, b):\n    return old + b",
            "def f(new, b):\n    return new + b",
            vec!["same"],
            vec!["other", "spread"],
        ),
        (
            "kotlin",
            vec![
                (
                    "M.kt",
                    "fun f(old: Int, b: Int): Int = old + b\nfun g(old: Int): Int = old\n",
                ),
                (
                    "U.kt",
                    "fun other() = f(1, g(old = 2))\nfun direct() = f(old = 1, b = 2)\n",
                ),
            ],
            "M.kt",
            "fun f(old: Int, b: Int): Int = old + b",
            "fun f(new: Int, b: Int): Int = new + b",
            vec!["direct"],
            vec!["other"],
        ),
        (
            "csharp",
            vec![
                (
                    "M.cs",
                    "public class M\n{\n    public static int F(int old, int b) => old + b;\n    public static int G(int old) => old;\n}\n",
                ),
                (
                    "U.cs",
                    "public static class U\n{\n    public static int Other() => M.F(1, M.G(old: 2));\n    public static int Direct() => M.F(old: 1, b: 2);\n}\n",
                ),
            ],
            "M.cs",
            "F(int old, int b) => old + b",
            "F(int @new, int b) => @new + b",
            vec!["Direct"],
            vec!["Other"],
        ),
        (
            "swift",
            vec![
                (
                    "M.swift",
                    "func f(old: Int, b: Int) -> Int { return old + b }\nfunc g(old: Int) -> Int { return old }\n",
                ),
                (
                    "U.swift",
                    "func outer() -> Int { return f(old: g(old: 1), b: 2) }\nfunc inner() -> Int { return f(new: g(old: 1), b: 2) }\n",
                ),
            ],
            "M.swift",
            "func f(old: Int, b: Int)",
            "func f(new: Int, b: Int)",
            vec!["outer"],
            vec!["inner"],
        ),
    ]
}

#[test]
fn a_named_argument_counts_only_in_the_calls_own_arguments_or_in_a_nested_call_of_the_same_name() {
    for (language, files, path, from, to, listed, unlisted) in nested_named_argument_cases() {
        let text = edited_text(&files, path, from, to);
        assert_has(&text, "parameter renamed: old → ");
        for caller in listed {
            assert_has(&text, &format!("`{caller}`  names parameter `old`"));
        }
        for caller in unlisted {
            assert!(
                !text.contains(&format!("`{caller}`")),
                "{language}: `{caller}` is listed in:\n{text}"
            );
        }
    }
}

type ToggleCase = (
    &'static str,
    Vec<(&'static str, &'static str)>,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

fn async_cases() -> Vec<ToggleCase> {
    vec![
        (
            "rust",
            vec![
                ("Cargo.toml", CARGO_TOML),
                (
                    "src/lib.rs",
                    "pub fn f(a: i32) -> i32 {\n    a\n}\n\npub fn g() -> i32 {\n    f(1)\n}\n",
                ),
            ],
            "src/lib.rs",
            "pub fn f",
            "pub async fn f",
            "src/lib.rs:6  `g`",
        ),
        (
            "python",
            vec![
                ("pkg/__init__.py", ""),
                ("pkg/m.py", "def f(a):\n    return a\n"),
                (
                    "pkg/u.py",
                    "from pkg.m import f\n\n\ndef g():\n    return f(1)\n",
                ),
            ],
            "pkg/m.py",
            "def f",
            "async def f",
            "pkg/u.py:5  `g`",
        ),
        (
            "javascript",
            vec![
                ("package.json", "{\"name\":\"fx\"}\n"),
                ("m.js", "export function f(a) {\n  return a;\n}\n"),
                (
                    "u.js",
                    "import { f } from \"./m\";\n\nexport function g() {\n  return f(1);\n}\n",
                ),
            ],
            "m.js",
            "export function f",
            "export async function f",
            "u.js:4  `g`",
        ),
        (
            "typescript",
            vec![
                ("package.json", "{\"name\":\"fx\"}\n"),
                (
                    "m.ts",
                    "export function f(a: number): Promise<number> {\n  return Promise.resolve(a);\n}\n",
                ),
                (
                    "u.ts",
                    "import { f } from \"./m\";\n\nexport function g(): Promise<number> {\n  return f(1);\n}\n",
                ),
            ],
            "m.ts",
            "export function f",
            "export async function f",
            "u.ts:4  `g`",
        ),
        (
            "csharp",
            vec![
                (
                    "M.cs",
                    "public static class M {\n    public static Task<int> F(int a) {\n        return null;\n    }\n}\n",
                ),
                (
                    "U.cs",
                    "public static class U {\n    public static Task<int> G() {\n        return M.F(1);\n    }\n}\n",
                ),
            ],
            "M.cs",
            "public static Task<int> F",
            "public static async Task<int> F",
            "U.cs:3  `G`",
        ),
    ]
}

#[test]
fn an_async_added_to_a_function_says_the_signature_changed_and_lists_every_call() {
    for (language, files, path, from, to, site) in async_cases() {
        let text = edited_text(&files, path, from, to);
        assert!(
            text.contains("): signature changed: async added"),
            "{language}:\n{text}"
        );
        assert!(text.contains(site), "{language}:\n{text}");
    }
}

#[test]
fn an_async_removed_from_a_function_says_the_signature_changed() {
    let text = edited_text(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/m.py", "async def f(a):\n    return a\n"),
            (
                "pkg/u.py",
                "from pkg.m import f\n\n\nasync def g():\n    return await f(1)\n",
            ),
        ],
        "pkg/m.py",
        "async def f",
        "def f",
    );
    assert_has(
        &text,
        "function `f` (pkg/m.py): signature changed: async removed",
    );
    assert_has(&text, "pkg/u.py:5  `g`");
}

#[test]
fn a_kotlin_suspend_added_to_a_function_says_the_signature_changed() {
    let text = edited_text(
        &[
            ("M.kt", "fun f(a: Int): Int = a\n"),
            ("U.kt", "fun g(): Int = f(1)\n"),
        ],
        "M.kt",
        "fun f",
        "suspend fun f",
    );
    assert_has(
        &text,
        "function `f` (M.kt): signature changed: suspend added",
    );
    assert_has(&text, "U.kt:1  `g`");
}

#[test]
fn a_static_removed_from_a_method_says_the_signature_changed_and_lists_every_call() {
    let cases: Vec<ToggleCase> = vec![
        (
            "java",
            vec![
                (
                    "M.java",
                    "public class M {\n    public static int f(int a) {\n        return a;\n    }\n}\n",
                ),
                (
                    "U.java",
                    "public class U {\n    public static int g() {\n        return M.f(1);\n    }\n}\n",
                ),
            ],
            "M.java",
            "public static int f",
            "public int f",
            "U.java:3  `g`",
        ),
        (
            "csharp",
            vec![
                (
                    "M.cs",
                    "public static class M {\n    public static int F(int a) {\n        return a;\n    }\n}\n",
                ),
                (
                    "U.cs",
                    "public static class U {\n    public static int G() {\n        return M.F(1);\n    }\n}\n",
                ),
            ],
            "M.cs",
            "public static int F",
            "public int F",
            "U.cs:3  `G`",
        ),
        (
            "typescript",
            vec![
                ("package.json", "{\"name\":\"fx\"}\n"),
                (
                    "m.ts",
                    "export class M {\n  static f(a: number): number {\n    return a;\n  }\n}\n",
                ),
                (
                    "u.ts",
                    "import { M } from \"./m\";\n\nexport function g(): number {\n  return M.f(1);\n}\n",
                ),
            ],
            "m.ts",
            "static f",
            "f",
            "u.ts:4  `g`",
        ),
        (
            "cpp",
            vec![(
                "m.cpp",
                "class M {\npublic:\n    static int f(int a) { return a; }\n};\n",
            )],
            "m.cpp",
            "static int f",
            "int f",
            "",
        ),
        (
            "php",
            vec![(
                "m.php",
                "<?php\nclass M {\n    public static function f($a) {\n        return $a;\n    }\n}\n",
            )],
            "m.php",
            "public static function f",
            "public function f",
            "",
        ),
    ];
    for (language, files, path, from, to, site) in cases {
        let text = edited_text(&files, path, from, to);
        assert!(
            text.contains("): signature changed: static removed"),
            "{language}:\n{text}"
        );
        assert!(text.contains(site), "{language}:\n{text}");
    }
}

#[test]
fn a_static_added_to_a_method_says_the_signature_changed() {
    let text = edited_text(
        &[
            (
                "M.java",
                "public class M {\n    public int f(int a) {\n        return a;\n    }\n}\n",
            ),
            (
                "U.java",
                "public class U {\n    public int g(M m) {\n        return m.f(1);\n    }\n}\n",
            ),
        ],
        "M.java",
        "public int f",
        "public static int f",
    );
    assert_has(
        &text,
        "method `M.f` (M.java): signature changed: static added",
    );
    assert_has(&text, "U.java:3  `g`");
}

const RUST_POINT: &str = "pub struct Point {\n    pub x: i32,\n    pub y: i32,\n}\n\nimpl Point {\n    pub fn origin() -> Self {\n        Self { x: 0, y: 0 }\n    }\n\n    pub fn sum(&self) -> i32 {\n        let Self { x, y } = self;\n        x + y\n    }\n}\n";
const RUST_POINT_USES: &str = "use crate::point::Point;\n\npub fn take(Point { x, y }: Point) -> i32 {\n    x + y\n}\n\npub fn keep(Point { x, .. }: Point) -> i32 {\n    x\n}\n\npub fn bind(p: Point) -> i32 {\n    let Point { x, y } = p;\n    x + y\n}\n\npub fn arm(p: Point) -> i32 {\n    match p {\n        Point { x, y } => x + y,\n    }\n}\n\npub fn named(Point { y: why, .. }: Point) -> i32 {\n    why\n}\n";

fn rust_point_repo() -> Repo {
    committed_repo(&[
        ("Cargo.toml", CARGO_TOML),
        ("src/lib.rs", "pub mod point;\npub mod uses;\n"),
        ("src/point.rs", RUST_POINT),
        ("src/uses.rs", RUST_POINT_USES),
    ])
}

#[test]
fn a_rust_struct_that_gains_a_field_lists_the_self_literals_and_the_patterns_without_a_rest() {
    let repo = rust_point_repo();
    write(
        &repo.root,
        "src/point.rs",
        &RUST_POINT.replace("    pub y: i32,\n", "    pub y: i32,\n    pub z: i32,\n"),
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Point` (src/point.rs): fields +1 (z), -0");
    for site in [
        "src/point.rs:9  ",
        "src/point.rs:13  ",
        "src/uses.rs:3  ",
        "src/uses.rs:12  ",
        "src/uses.rs:18  ",
    ] {
        assert_has(&text, site);
    }
    assert_lacks(&text, "src/uses.rs:7  ");
    assert_lacks(&text, "src/uses.rs:22  ");
}

#[test]
fn a_rust_struct_that_loses_a_field_lists_the_self_literals_and_the_patterns_that_name_it() {
    let repo = rust_point_repo();
    write(
        &repo.root,
        "src/point.rs",
        &RUST_POINT.replace("    pub y: i32,\n", ""),
    );
    let text = text_of(&repo);
    assert_has(&text, "struct `Point` (src/point.rs): fields +0, -1 (y)");
    for site in [
        "src/point.rs:7  ",
        "src/point.rs:11  ",
        "src/uses.rs:3  ",
        "src/uses.rs:12  ",
        "src/uses.rs:18  ",
        "src/uses.rs:22  ",
    ] {
        assert_has(&text, site);
    }
    assert_lacks(&text, "src/uses.rs:7  ");
}

#[test]
fn a_python_dataclass_class_variable_is_tracked_without_changing_construction() {
    for declaration in [
        "c: ClassVar[int]",
        "c: typing.ClassVar[int] = 0",
        "c: \"ClassVar[int]\"",
        "c: ClassVar",
    ] {
        let repo = committed_repo(&[("shapes.py", PY_REC), ("use.py", PY_REC_USES)]);
        write(
            &repo.root,
            "shapes.py",
            &format!("{PY_REC}    {declaration}\n"),
        );
        let text = text_of(&repo);
        assert!(text.contains("fields +1 (c), -0"), "{declaration}:\n{text}");
        assert_lacks(&text, "Not in your diff");
    }
}

#[test]
fn a_dataclass_field_call_has_a_default_only_with_default_or_default_factory() {
    let cases = [
        ("field(metadata={})", true),
        ("field(kw_only=True)", true),
        ("dataclasses.field(repr=False)", true),
        ("field(default=0, kw_only=True)", false),
        ("field(default_factory=list)", false),
        ("field(\n        default=3,\n    )", false),
        ("field(init=False)", false),
        ("0", false),
    ];
    for (value, listed) in cases {
        let repo = committed_repo(&[("shapes.py", PY_REC), ("use.py", PY_REC_USES)]);
        write(
            &repo.root,
            "shapes.py",
            &format!("{PY_REC}    c: int = {value}\n"),
        );
        let text = text_of(&repo);
        assert!(text.contains("fields +1 (c), -0"), "{value}:\n{text}");
        assert_eq!(
            text.contains("use.py:5  `make`"),
            listed,
            "{value}:\n{text}"
        );
    }
}

const JAVA_RECORD_USES: &str = "public class Use {\n    public Point one() {\n        return new Point(1);\n    }\n    public Point two() {\n        return new Point(1, 2);\n    }\n}\n";

#[test]
fn a_java_record_with_an_extra_constructor_lists_only_the_calls_with_the_old_component_count() {
    let text = edited_text(
        &[
            (
                "Point.java",
                "public record Point(int x, int y) {\n    public Point(int x) {\n        this(x, 0);\n    }\n}\n",
            ),
            ("Use.java", JAVA_RECORD_USES),
        ],
        "Point.java",
        "int x, int y) {",
        "int x, int y, int z) {",
    );
    assert_has(&text, "fields +1 (z), -0");
    assert_has(&text, "Use.java:6  `two`");
    assert_has(&text, "Point.java:3");
    assert_lacks(&text, "`one`");
}

#[test]
fn a_python_lambda_that_moved_to_another_line_is_no_rename() {
    let text = edited_text(
        &[(
            "app.py",
            "def helper():\n    return 1\n\n\ndef build(register):\n    register(lambda value: value)\n    return register\n",
        )],
        "app.py",
        "def helper():\n    return 1\n\n\n",
        "",
    );
    assert_has(&text, "function `helper` (app.py): removed");
    assert_lacks(&text, "lambda_");
}

#[test]
fn a_generic_java_record_lists_the_constructions_with_type_arguments() {
    let text = edited_text(
        &[
            ("Pair.java", "public record Pair<A, B>(A a, B b) {\n}\n"),
            (
                "Use.java",
                "public class Use {\n    static Object diamond() {\n        return new Pair<>(\"a\", 1);\n    }\n\n    static Object typed() {\n        return new Pair<String, Integer>(\"a\", 1);\n    }\n}\n",
            ),
        ],
        "Pair.java",
        "(A a, B b)",
        "(A a, B b, int c)",
    );
    assert_has(&text, "Use.java:3  `diamond`");
    assert_has(&text, "Use.java:7  `typed`");
}

#[test]
fn a_this_call_with_another_count_than_the_old_component_count_is_no_site() {
    let repo = committed_repo(&[
        (
            "Point.java",
            "public record Point(int x, int y) {\n    public Point(int x) {\n        this(x, 0);\n    }\n}\n",
        ),
        ("Use.java", JAVA_RECORD_USES),
    ]);
    write(
        &repo.root,
        "Point.java",
        "public record Point(int x, int y, int z) {\n    public Point(int x) {\n        this(x, 0, 0);\n    }\n}\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "fields +1 (z), -0");
    assert_has(&text, "Use.java:6  `two`");
    assert_lacks(&text, "Point.java:3");
}

#[test]
fn a_this_call_with_the_old_component_count_is_a_site_of_a_record_that_loses_a_component() {
    let text = edited_text(
        &[
            (
                "Point.java",
                "public record Point(int x, int y) {\n    public Point(String s) {\n        this(1, 2);\n    }\n}\n",
            ),
            ("Use.java", JAVA_RECORD_USES),
        ],
        "Point.java",
        "int x, int y) {",
        "int x) {",
    );
    assert_has(&text, "fields +0, -1 (y)");
    assert_has(&text, "Point.java:3");
}

#[test]
fn go_result_lists_with_and_without_names_are_no_shape_change() {
    let files = [
        ("go.mod", GO_MOD),
        (
            "geom/geom.go",
            "package geom\n\nfunc F(a int) (int, error) {\n\treturn a, nil\n}\n\nfunc H(a int) (int, int) {\n\treturn a, a\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport \"fx/geom\"\n\nfunc Run() int {\n\tn, _ := geom.F(1)\n\treturn n\n}\n",
        ),
    ];
    let named = edited_text(&files, "geom/geom.go", "(int, error)", "(n int, err error)");
    assert_has(&named, "no symbol changed shape.");
    let grouped = edited_text(&files, "geom/geom.go", "(int, int)", "(x, y int)");
    assert_has(&grouped, "no symbol changed shape.");
    let retyped = edited_text(
        &files,
        "geom/geom.go",
        "(int, error)",
        "(n int, err string)",
    );
    assert_has(&retyped, "function `F` (geom/geom.go): signature changed");
}

#[test]
fn java_and_kotlin_parameter_annotations_are_no_shape_change() {
    let java = [
        (
            "Calc.java",
            "public class Calc {\n    public int add(int a, int b) {\n        return a + b;\n    }\n}\n",
        ),
        (
            "Main.java",
            "public class Main {\n    public int run(Calc c) {\n        return c.add(1, 2);\n    }\n}\n",
        ),
    ];
    let text = edited_text(
        &java,
        "Calc.java",
        "add(int a, int b)",
        "add(@Deprecated int a, @Size(min = 1, max = 2) final int b)",
    );
    assert_has(&text, "no symbol changed shape.");
    let kotlin = [
        ("M.kt", "fun add(a: Int, b: Int): Int = a + b\n"),
        ("U.kt", "fun run(): Int = add(1, 2)\n"),
    ];
    let text = edited_text(
        &kotlin,
        "M.kt",
        "add(a: Int, b: Int)",
        "add(@Suppress(\"x\") a: Int, @Foo b: Int)",
    );
    assert_has(&text, "no symbol changed shape.");
}

#[test]
fn a_rust_ignored_parameter_that_gets_a_name_is_no_shape_change() {
    let files = [
        ("Cargo.toml", CARGO_TOML),
        (
            "src/lib.rs",
            "pub fn f(a: i32, _: u8) -> i32 {\n    a\n}\n\npub fn g() -> i32 {\n    f(1, 2)\n}\n",
        ),
    ];
    let named = edited_text(&files, "src/lib.rs", "_: u8", "_unused: u8");
    assert_has(&named, "no symbol changed shape.");
    let retyped = edited_text(&files, "src/lib.rs", "_: u8", "_: u16");
    assert_has(&retyped, "function `f` (src/lib.rs): signature changed");
}

#[test]
fn a_named_argument_of_another_call_on_the_same_line_does_not_list_the_call() {
    let repo = committed_repo(&[
        (
            "m.py",
            "def scale(value, factor=1):\n    return value * factor\n\n\ndef other(**kw):\n    return 0\n",
        ),
        (
            "use.py",
            "from m import scale, other\n\n\ndef mixed():\n    return scale(1, 2) + other(factor=3)\n\n\ndef keyword():\n    return scale(1, factor=2)\n",
        ),
    ]);
    write(
        &repo.root,
        "m.py",
        "def scale(value, ratio=1):\n    return value * ratio\n\n\ndef other(**kw):\n    return 0\n",
    );
    let text = text_of(&repo);
    assert_has(&text, "use.py:9  `keyword`  names parameter `factor`");
    assert_lacks(&text, "`mixed`");
}

#[test]
fn a_call_that_spreads_its_arguments_is_unresolved_when_the_parameters_change() {
    let text = edited_text(
        &[
            ("pkg/__init__.py", ""),
            ("pkg/m.py", "def f(a):\n    return a\n"),
            (
                "pkg/u.py",
                "from pkg.m import f\n\n\ndef g(args):\n    return f(*args)\n\n\ndef h(kw):\n    return f(**kw)\n\n\ndef k():\n    return f(1)\n",
            ),
        ],
        "pkg/m.py",
        "def f(a)",
        "def f(a, b)",
    );
    assert_has(&text, "function `f` (pkg/m.py): parameters 1 → 2");
    assert_has(&text, "pkg/u.py:13  `k`  passes 1 arg; takes 2");
    assert_has(&text, "Unresolved outside the diff: 2");
    assert_lacks(&text, "`g`");
    assert_lacks(&text, "`h`");
}

#[test]
fn a_removed_go_method_counts_the_calls_that_other_types_with_the_name_can_take() {
    let text = edited_text(
        &[
            ("go.mod", GO_MOD),
            (
                "geom/g.go",
                "package geom\n\ntype A struct{}\n\nfunc (a A) Norm() int {\n\treturn 1\n}\n\ntype B struct{}\n\nfunc (b B) Norm() int {\n\treturn 2\n}\n\nfunc Norm() int {\n\treturn 3\n}\n",
            ),
            (
                "use/u.go",
                "package use\n\nimport \"fx/geom\"\n\nfunc UseA(a geom.A) int {\n\treturn a.Norm()\n}\n\nfunc UseB(b geom.B) int {\n\treturn b.Norm()\n}\n\nfunc UseFree() int {\n\treturn geom.Norm()\n}\n",
            ),
        ],
        "geom/g.go",
        "func (a A) Norm() int {\n\treturn 1\n}\n\n",
        "",
    );
    assert_has(&text, "method `A.Norm` (geom/g.go): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
}

#[test]
fn an_async_added_to_an_arrow_function_says_the_signature_changed() {
    let text = edited_text(
        &[
            ("package.json", "{\"name\":\"fx\"}\n"),
            ("m.js", "export const f = (a) => a;\n"),
            (
                "u.js",
                "import { f } from \"./m\";\n\nexport function g() {\n  return f(1);\n}\n",
            ),
        ],
        "m.js",
        "export const f = (a)",
        "export const f = async (a)",
    );
    assert_has(&text, "function `f` (m.js): signature changed: async added");
    assert_has(&text, "u.js:4  `g`");
}

const CSHARP_ATTRIBUTE_FILES: [(&str, &str); 2] = [
    (
        "M.cs",
        "public class M\n{\n    [HttpGet(\"/a\")]\n    public int H(int a) => a;\n}\n",
    ),
    ("U.cs", "public class U { public int X(M m) => m.H(1); }\n"),
];

#[test]
fn a_csharp_attribute_added_removed_or_on_a_parameter_is_no_shape_change() {
    let removed = edited_text(
        &CSHARP_ATTRIBUTE_FILES,
        "M.cs",
        "    [HttpGet(\"/a\")]\n",
        "",
    );
    assert_has(&removed, "no symbol changed shape.");
    let added = edited_text(
        &CSHARP_ATTRIBUTE_FILES,
        "M.cs",
        "    [HttpGet(\"/a\")]\n",
        "    [HttpGet(\"/a\")]\n    [Obsolete, Route(\"[controller]\")] [NonAction]\n",
    );
    assert_has(&added, "no symbol changed shape.");
    let parameter = edited_text(
        &CSHARP_ATTRIBUTE_FILES,
        "M.cs",
        "H(int a)",
        "H([Range(0, 1)] int a)",
    );
    assert_has(&parameter, "no symbol changed shape.");
    let retyped = edited_text(
        &CSHARP_ATTRIBUTE_FILES,
        "M.cs",
        "    [HttpGet(\"/a\")]\n    public int H",
        "    [Obsolete]\n    public long H",
    );
    assert_has(&retyped, "method `M.H` (M.cs): signature changed");
}

#[test]
fn a_csharp_attribute_that_holds_the_name_of_the_method_does_not_hide_an_async() {
    let text = edited_text(
        &[
            (
                "M.cs",
                "public class M\n{\n    [Authorize]\n    public Task Authorize(int a) { return null; }\n}\n",
            ),
            (
                "U.cs",
                "public class U { public Task X(M m) => m.Authorize(1); }\n",
            ),
        ],
        "M.cs",
        "public Task Authorize",
        "public async Task Authorize",
    );
    assert_has(
        &text,
        "method `M.Authorize` (M.cs): signature changed: async added",
    );
}

#[test]
fn a_php_attribute_added_or_on_a_parameter_is_no_shape_change() {
    let files = [
        (
            "M.php",
            "<?php\nclass M {\n    public function f(int $a): int { return $a; }\n}\n",
        ),
        ("U.php", "<?php\nfunction x(M $m) { return $m->f(1); }\n"),
    ];
    let added = edited_text(
        &files,
        "M.php",
        "    public function f",
        "    #[Pure]\n    #[Route('/a', name: 'a')]\n    public function f",
    );
    assert_has(&added, "no symbol changed shape.");
    let parameter = edited_text(
        &files,
        "M.php",
        "f(int $a)",
        "f(#[SensitiveParameter] int $a)",
    );
    assert_has(&parameter, "no symbol changed shape.");
    let retyped = edited_text(
        &files,
        "M.php",
        "    public function f(int $a): int",
        "    #[Pure]\n    public function f(int $a): string",
    );
    assert_has(&retyped, "method `M.f` (M.php): signature changed");
}

#[test]
fn a_cpp_static_is_a_call_shape_word_only_for_a_member_of_a_class_or_struct() {
    let file_scope = "static int helper(int a) { return a; }\nint use() { return helper(1); }\n";
    let removed = edited_text(
        &[("a.cpp", file_scope)],
        "a.cpp",
        "static int helper",
        "int helper",
    );
    assert_has(&removed, "no symbol changed shape.");
    let added = edited_text(
        &[(
            "a.cpp",
            "int helper(int a) { return a; }\nint use() { return helper(1); }\n",
        )],
        "a.cpp",
        "int helper(int a)",
        "static int helper(int a)",
    );
    assert_has(&added, "no symbol changed shape.");
    let namespaced = edited_text(
        &[(
            "a.cpp",
            "namespace n {\nstatic int helper(int a) { return a; }\n}\nint use() { return n::helper(1); }\n",
        )],
        "a.cpp",
        "static int helper",
        "int helper",
    );
    assert_has(&namespaced, "no symbol changed shape.");
    for keyword in ["struct", "class"] {
        let member = format!(
            "{keyword} S {{\npublic:\n    static int helper(int a) {{ return a; }}\n}};\nint use() {{ return S::helper(1); }}\n"
        );
        let text = edited_text(
            &[("a.cpp", member.as_str())],
            "a.cpp",
            "static int helper",
            "int helper",
        );
        assert_has(&text, "): signature changed: static removed");
    }
}

const TS_SCHEMAS_WITH_MEMBER: &str = "declare function install(inst: object, methods: object): void;\ndeclare function make(name: string, init: (inst: object) => void): object;\n\nexport const ZodType = make(\"ZodType\", (inst) => {\n  install(inst, {\n    array() {\n      return array(this);\n    },\n    keep() {\n      return 1;\n    },\n  });\n});\n\nexport function array(element: unknown) {\n  return [element];\n}\n";
const TS_SCHEMAS_WITHOUT_MEMBER: &str = "declare function install(inst: object, methods: object): void;\ndeclare function make(name: string, init: (inst: object) => void): object;\n\nexport const ZodType = make(\"ZodType\", (inst) => {\n  install(inst, {\n    keep() {\n      return 1;\n    },\n  });\n});\n\nexport function array(element: unknown) {\n  return [element];\n}\n";
const TS_SCHEMAS_INDEX: &str = "export * from \"./schemas\";\n";
const TS_SCHEMAS_USE: &str =
    "import * as z from \"./index\";\n\nexport function usesArray() {\n  return z.array(1);\n}\n";

#[test]
fn a_removed_object_literal_method_does_not_list_a_namespace_call_of_the_module_function_with_its_name()
 {
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        ("src/schemas.ts", TS_SCHEMAS_WITH_MEMBER),
        ("src/index.ts", TS_SCHEMAS_INDEX),
        ("src/use.ts", TS_SCHEMAS_USE),
    ]);
    write(&repo.root, "src/schemas.ts", TS_SCHEMAS_WITHOUT_MEMBER);
    let text = text_of(&repo);
    assert_has(&text, "(src/schemas.ts): removed");
    assert_lacks(&text, "usesArray");
}

const TS_CODES: &str = "export const Codes = { invalid: \"invalid\" } as const;\n\nexport type Code = keyof typeof Codes;\n\nconst local = 1;\n\nexport function keep() {\n  return local;\n}\n";
const TS_CODES_WITHOUT_BINDINGS: &str = "export function keep() {\n  return 1;\n}\n";
const TS_CODES_USE: &str = "import { Codes } from \"./codes\";\nimport type { Code } from \"./codes\";\n\nexport function usesCodes(code: Code) {\n  return Codes.invalid === code;\n}\n";

fn ts_codes_repo() -> Repo {
    committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        ("src/codes.ts", TS_CODES),
        ("src/use.ts", TS_CODES_USE),
    ])
}

#[test]
fn a_removed_exported_constant_and_type_alias_list_the_uses_in_another_file() {
    let repo = ts_codes_repo();
    write(&repo.root, "src/codes.ts", TS_CODES_WITHOUT_BINDINGS);
    let text = text_of(&repo);
    assert_has(&text, "`Codes` (src/codes.ts): removed");
    assert_has(&text, "`Code` (src/codes.ts): removed");
    assert_has(&text, "src/use.ts:4  `usesCodes`");
    assert_has(&text, "src/use.ts:5  `usesCodes`");
    assert_lacks(&text, "`local`");
}

#[test]
fn a_type_alias_that_becomes_an_interface_of_the_same_name_is_no_change() {
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        ("src/shape.ts", "export type Shape = { a: number };\n"),
        (
            "src/use.ts",
            "import type { Shape } from \"./shape\";\n\nexport function area(s: Shape) {\n  return s.a;\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/shape.ts",
        "export interface Shape {\n  a: number;\n}\n",
    );
    let text = text_of(&repo);
    assert_lacks(&text, "Shape");
}

const PY_CONFTEST: &str = "import pytest\n\n\n@pytest.fixture\ndef limit_loader():\n    return 1\n\n\n@pytest.fixture\ndef keep():\n    return 2\n";
const PY_CONFTEST_WITHOUT_FIXTURE: &str =
    "import pytest\n\n\n@pytest.fixture\ndef keep():\n    return 2\n";
const PY_NEARER_CONFTEST: &str =
    "import pytest\n\n\n@pytest.fixture\ndef limit_loader():\n    return 3\n";

#[test]
fn a_removed_pytest_fixture_lists_the_tests_that_take_it_and_their_reads_of_it() {
    let repo = committed_repo(&[
        ("tests/conftest.py", PY_CONFTEST),
        (
            "tests/test_a.py",
            "def test_uses(limit_loader):\n    assert limit_loader == 1\n",
        ),
        ("tests/sub/conftest.py", PY_NEARER_CONFTEST),
        (
            "tests/sub/test_b.py",
            "def test_other(limit_loader):\n    assert limit_loader == 3\n",
        ),
    ]);
    write(&repo.root, "tests/conftest.py", PY_CONFTEST_WITHOUT_FIXTURE);
    let text = text_of(&repo);
    assert_has(
        &text,
        "function `limit_loader` (tests/conftest.py): removed",
    );
    assert_has(
        &text,
        "tests/test_a.py:1  `test_uses`  takes the fixture `limit_loader`",
    );
    assert_has(&text, "tests/test_a.py:2  `test_uses`");
    assert_lacks(&text, "test_other");
}

#[test]
fn a_removed_constant_and_type_alias_of_one_name_report_once_with_an_import_that_names_the_ts_file()
{
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        (
            "src/issues.ts",
            "export const IssueCode = { custom: \"custom\" } as const;\n\nexport type IssueCode = keyof typeof IssueCode;\n\nexport function keep() {\n  return 1;\n}\n",
        ),
        (
            "src/use.ts",
            "import { IssueCode } from \"./issues.ts\";\n\nexport function usesCode(code: IssueCode) {\n  return IssueCode.custom === code;\n}\n",
        ),
    ]);
    write(&repo.root, "src/issues.ts", TS_CODES_WITHOUT_BINDINGS);
    let text = text_of(&repo);
    assert_has(&text, "1 shape change:");
    assert_has(&text, "`IssueCode` (src/issues.ts): removed");
    assert_has(
        &text,
        "src/use.ts:1  (no enclosing symbol)  imports `IssueCode`",
    );
    assert_has(&text, "src/use.ts:4  `usesCode`");
}

#[test]
fn a_re_export_of_a_deleted_typescript_file_proves_the_call_of_its_removed_function() {
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        (
            "src/lib/util.ts",
            "export function helper() {\n  return 1;\n}\n",
        ),
        (
            "mirror/util.ts",
            "export function helper() {\n  return 2;\n}\n",
        ),
        ("src/index.ts", "export * from \"./lib/util.ts\";\n"),
        (
            "src/use.ts",
            "import { helper } from \"./index.ts\";\n\nexport function usesHelper() {\n  return helper();\n}\n",
        ),
    ]);
    fs::remove_file(repo.root.join("src/lib/util.ts")).unwrap();
    let text = text_of(&repo);
    assert_has(&text, "function `helper` (src/lib/util.ts): removed");
    assert_has(
        &text,
        "Not in your diff, proven (1):\n      src/use.ts:4  `usesHelper`",
    );
}

#[test]
fn a_deleted_javascript_twin_of_a_typescript_file_lists_the_calls_of_its_javascript_importers() {
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        (
            "src/util.ts",
            "export function helper(): number {\n  return 1;\n}\n",
        ),
        (
            "src/util.js",
            "export function helper() {\n  return 1;\n}\n",
        ),
        (
            "src/use.js",
            "import { helper } from \"./util\";\n\nexport function fromScript() {\n  return helper();\n}\n",
        ),
        (
            "src/use.ts",
            "import { helper } from \"./util\";\n\nexport function fromSource() {\n  return helper();\n}\n",
        ),
    ]);
    fs::remove_file(repo.root.join("src/util.js")).unwrap();
    let text = text_of(&repo);
    assert_has(&text, "function `helper` (src/util.js): removed");
    assert_has(&text, "src/use.js:4  `fromScript`");
    assert_lacks(&text, "fromSource");
}

#[test]
fn a_removed_csharp_fallback_lists_a_use_beside_a_using_under_the_condition() {
    let repo = committed_repo(&[
        (
            "src/Utilities/ConcurrentDictionary.cs",
            "#if !HAVE_CONCURRENT\nnamespace App.Serialization\n{\n    internal sealed class ConcurrentDictionary<TKey, TValue>\n    {\n    }\n}\n#endif\n",
        ),
        (
            "src/Utilities/Store.cs",
            "using System;\n#if HAVE_CONCURRENT\nusing System.Collections.Concurrent;\n#endif\nusing App.Serialization;\n\nnamespace App.Utilities\n{\n    internal class Store\n    {\n        private ConcurrentDictionary<string, int> _store;\n    }\n}\n",
        ),
        (
            "src/Utilities/Always.cs",
            "using System;\nusing System.Collections.Concurrent;\nusing App.Serialization;\n\nnamespace App.Utilities\n{\n    internal class Always\n    {\n        private ConcurrentDictionary<string, int> _always;\n    }\n}\n",
        ),
    ]);
    fs::remove_file(repo.root.join("src/Utilities/ConcurrentDictionary.cs")).unwrap();
    let text = text_of(&repo);
    assert_has(&text, "src/Utilities/Store.cs:11");
    assert_lacks(&text, "src/Utilities/Always.cs");
}

#[test]
fn a_removed_python_binding_set_in_both_branches_of_an_if_is_reported_once() {
    let repo = committed_repo(&[
        (
            "compat.py",
            "import sys\n\nPY2 = sys.version_info[0] == 2\n\nif PY2:\n    text_type = unicode\nelse:\n    text_type = str\n",
        ),
        (
            "use.py",
            "from compat import text_type\n\n\ndef run(x):\n    return text_type(x)\n",
        ),
    ]);
    fs::remove_file(repo.root.join("compat.py")).unwrap();
    let text = text_of(&repo);
    assert_eq!(
        text.matches("variable `text_type` (compat.py): removed")
            .count(),
        1,
        "{text}"
    );
    assert_has(&text, "use.py:5  `run`");
}

const JAVA_CHECKS: &str = "package p.util;\n\npublic class Checks {\n    public static void check(boolean b) {\n    }\n\n    public static void keep() {\n    }\n}\n";
const JAVA_CHECKS_WITHOUT_CHECK: &str =
    "package p.util;\n\npublic class Checks {\n    public static void keep() {\n    }\n}\n";
const JAVA_CHECKS_USE: &str = "package p;\n\nimport p.util.Checks;\nimport static p.util.Checks.check;\n\npublic class Use {\n    void viaClass() {\n        Checks.keep();\n    }\n\n    void viaImport() {\n        check(false);\n    }\n}\n";

fn java_checks_repo() -> Repo {
    committed_repo(&[
        ("src/main/java/p/util/Checks.java", JAVA_CHECKS),
        ("src/main/java/p/Use.java", JAVA_CHECKS_USE),
    ])
}

#[test]
fn a_removed_java_class_lists_its_import_and_the_static_import_of_its_member() {
    let repo = java_checks_repo();
    fs::remove_file(repo.root.join("src/main/java/p/util/Checks.java")).unwrap();
    let text = text_of(&repo);
    assert_has(
        &text,
        "class `Checks` (src/main/java/p/util/Checks.java): removed",
    );
    assert_has(
        &text,
        "src/main/java/p/Use.java:3  (no enclosing symbol)  imports `Checks`",
    );
    assert_has(
        &text,
        "src/main/java/p/Use.java:4  (no enclosing symbol)  imports `Checks`",
    );
}

#[test]
fn a_removed_java_static_method_lists_its_static_import() {
    let repo = java_checks_repo();
    write(
        &repo.root,
        "src/main/java/p/util/Checks.java",
        JAVA_CHECKS_WITHOUT_CHECK,
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "method `Checks.check` (src/main/java/p/util/Checks.java): removed",
    );
    assert_has(
        &text,
        "src/main/java/p/Use.java:4  (no enclosing symbol)  imports `check`",
    );
    assert_has(
        &text,
        "Not in your diff, proven (1):\n      src/main/java/p/Use.java:12  `viaImport`",
    );
    assert_lacks(&text, "Use.java:3");
}

const JS_REQUEST: &str = "var req = Object.create(null);\n\nmodule.exports = req;\n\nreq.param = function param(name) {\n  return name;\n};\n\nreq.keep = function keep() {\n  return 1;\n};\n";
const JS_REQUEST_WITHOUT_PARAM: &str = "var req = Object.create(null);\n\nmodule.exports = req;\n\nreq.keep = function keep() {\n  return 1;\n};\n";

#[test]
fn a_removed_javascript_method_keeps_the_object_it_was_assigned_on() {
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        ("lib/request.js", JS_REQUEST),
        (
            "lib/application.js",
            "var app = exports = module.exports = {};\n\napp.param = function param(name) {\n  return name;\n};\n",
        ),
        (
            "test/use.js",
            "function handler(req, res) {\n  return req.param('name');\n}\n\nmodule.exports = handler;\n",
        ),
    ]);
    write(&repo.root, "lib/request.js", JS_REQUEST_WITHOUT_PARAM);
    let text = text_of(&repo);
    assert_has(&text, "`param` (lib/request.js): removed");
    assert_has(&text, "test/use.js:2  `handler`");
}

#[test]
fn a_removed_typescript_type_skips_an_import_of_a_mirror_copy() {
    let parse_util =
        "export type ParseParams = { path: string[] };\n\nexport type Keep = string;\n";
    let types = "import { ParseParams } from \"./helpers/parseUtil\";\n\nexport function parse(p: ParseParams) {\n  return p;\n}\n";
    let repo = committed_repo(&[
        ("package.json", "{\"name\": \"demo\"}\n"),
        ("src/helpers/parseUtil.ts", parse_util),
        ("src/types.ts", types),
        ("deno/lib/helpers/parseUtil.ts", parse_util),
        ("deno/lib/types.ts", types),
    ]);
    write(
        &repo.root,
        "deno/lib/helpers/parseUtil.ts",
        "export type Keep = string;\n",
    );
    let text = text_of(&repo);
    assert_has(
        &text,
        "deno/lib/types.ts:1  (no enclosing symbol)  imports `ParseParams`",
    );
    assert_lacks(&text, "src/types.ts:1");
}

#[test]
fn a_removed_csharp_type_skips_a_use_whose_file_does_not_open_its_namespace() {
    let repo = committed_repo(&[
        (
            "src/Utilities/Cache.cs",
            "namespace App.Utilities\n{\n    internal class Cache\n    {\n    }\n}\n",
        ),
        (
            "src/Utilities/Other.cs",
            "namespace App.Utilities\n{\n    internal class Other\n    {\n    }\n}\n",
        ),
        (
            "src/Tests/GameObject.cs",
            "using System.Collections.Concurrent;\n\nnamespace App.Tests\n{\n    public class GameObject\n    {\n        public Cache Components;\n    }\n}\n",
        ),
        (
            "src/Tests/Opened.cs",
            "using App.Utilities;\n\nnamespace App.Tests\n{\n    public class Opened\n    {\n        public Cache Components;\n    }\n}\n",
        ),
    ]);
    fs::remove_file(repo.root.join("src/Utilities/Cache.cs")).unwrap();
    let text = text_of(&repo);
    assert_has(
        &text,
        "class `App.Utilities.Cache` (src/Utilities/Cache.cs): removed",
    );
    assert_has(&text, "src/Tests/Opened.cs:7");
    assert_lacks(&text, "GameObject.cs");
}

#[test]
fn a_go_method_whose_type_is_in_another_file_is_named_with_its_type() {
    let text = edited_text(
        &[
            ("go.mod", GO_MOD),
            (
                "command.go",
                "package fx\n\ntype Command struct{}\n\nfunc (c *Command) Run() {\n\tc.execute(1)\n}\n",
            ),
            (
                "exec.go",
                "package fx\n\nfunc (c *Command) execute(a int) {}\n",
            ),
        ],
        "exec.go",
        "execute(a int)",
        "execute(a, b int)",
    );
    assert_has(
        &text,
        "method `Command.execute` (exec.go): parameters 1 → 2",
    );
    assert_has(&text, "command.go:6  `Run`  passes 1 arg; takes 2");
}

#[test]
fn a_removed_go_method_counts_the_calls_through_an_alias_of_its_type() {
    let text = edited_text(
        &[
            ("go.mod", GO_MOD),
            (
                "geom/g.go",
                "package geom\n\ntype A struct{}\n\nfunc (a A) Norm() int {\n\treturn 1\n}\n\ntype B struct{}\n\nfunc (b B) Norm() int {\n\treturn 2\n}\n\ntype AA = A\n",
            ),
            (
                "use/u.go",
                "package use\n\nimport \"fx/geom\"\n\ntype Local = geom.A\n\nfunc UseAlias(a geom.AA) int {\n\treturn a.Norm()\n}\n\nfunc UseLocal(l Local) int {\n\treturn l.Norm()\n}\n\nfunc UseB(b geom.B) int {\n\treturn b.Norm()\n}\n",
            ),
        ],
        "geom/g.go",
        "func (a A) Norm() int {\n\treturn 1\n}\n\n",
        "",
    );
    assert_has(&text, "method `A.Norm` (geom/g.go): removed");
    assert_has(&text, "Unresolved outside the diff: 2");
}
