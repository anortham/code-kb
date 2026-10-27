use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use code_kb_core::{
    db::open_read_only,
    edit_check::{EditCheck, ShapeChange},
    formatters::stop_sites,
    models::SiteLabel,
    ops::edit_check_op,
    reference_report::{reference_report, select_reference_target},
    safe_tempdir,
    sync::scan_workspace,
    workspace::Workspace,
};
use tempfile::TempDir;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: TempDir,
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
    let path = root.join(path);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn committed_repo(files: &[(&str, &str)]) -> Repo {
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

fn check(repo: &Repo) -> EditCheck {
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    edit_check_op(&workspace, &repo.db, &conn, None).expect("edit check failed")
}

fn changed<'a>(check: &'a EditCheck, display: &str) -> &'a ShapeChange {
    check
        .changes
        .iter()
        .find(|change| change.display == display)
        .unwrap_or_else(|| panic!("missing `{display}` change in {check:?}"))
}

#[test]
fn a_removed_python_member_reports_its_read_without_claiming_another_owner() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\n\n@dataclass\nclass Box:\n    value: int\n    keep: int = 0\n\n@dataclass\nclass Other:\n    value: int\n",
        ),
        (
            "use.py",
            "from shapes import Box, Other\n\ndef read_box(box: Box):\n    return box.value\n\ndef read_other(other: Other):\n    return other.value\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\n\n@dataclass\nclass Box:\n    keep: int = 0\n\n@dataclass\nclass Other:\n    value: int\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Box");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "use.py" && site.caller == "read_box"),
        "missing removed field read in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "use.py" || site.caller != "read_other"),
        "reported a same-name member from another owner: {change:?}"
    );
}

#[test]
fn removing_an_fsharp_record_field_reports_only_its_typed_literal() {
    let repo = committed_repo(&[(
        "Point.fs",
        "namespace Probe\ntype Point = { X: int; Y: int }\ntype Other = { Y: int }\n\nmodule Usage =\n    let point : Point = { X = 1; Y = 2 }\n    let pointAscribed = { X = 1; Y = 2 } : Point\n    let other : Other = { Y = 3 }\n",
    )]);
    write(
        &repo.root,
        "Point.fs",
        "namespace Probe\ntype Point = { X: int }\ntype Other = { Y: int }\n\nmodule Usage =\n    let point : Point = { X = 1; Y = 2 }\n    let pointAscribed = { X = 1; Y = 2 } : Point\n    let other : Other = { Y = 3 }\n",
    );

    let check = check(&repo);
    let change = check
        .changes
        .iter()
        .find(|change| change.display.ends_with("Point"))
        .unwrap_or_else(|| panic!("missing Point field change in {check:?}"));

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "Point.fs" && site.line == 6),
        "missing typed F# record literal in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "Point.fs" && site.line == 7),
        "missing expression-ascribed F# record literal in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "Point.fs" || site.line != 8),
        "reported a same-name field from another record: {change:?}"
    );
}

#[test]
fn removing_a_python_override_does_not_prove_a_read_break_when_an_ancestor_defines_it() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\n\n@dataclass\nclass Base:\n    value: int\n\n@dataclass\nclass Box(Base):\n    value: int\n    keep: int = 0\n",
        ),
        (
            "use.py",
            "from shapes import Box\n\ndef read_box(box: Box):\n    return box.value\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\n\n@dataclass\nclass Base:\n    value: int\n\n@dataclass\nclass Box(Base):\n    keep: int = 0\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Box");

    assert!(
        change
            .proven
            .iter()
            .all(|site| site.path != "use.py" || site.caller != "read_box"),
        "reported a field read still satisfied by an ancestor: {change:?}"
    );
}

#[test]
fn a_removed_python_instance_attribute_reports_a_read_from_its_extracted_class_owner() {
    let repo = committed_repo(&[(
        "sample.py",
        "class Box:\n    class_value: int = 1\n\n    def __init__(self):\n        self.value = 1\n\n    def read(self):\n        return self.value + self.class_value\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let owner: (String, String) = conn
        .query_row(
            "SELECT parent.name, parent.kind FROM symbols member
             JOIN symbols parent ON parent.symbol_id = member.parent_symbol_id
             WHERE member.path = 'sample.py' AND member.name = 'value' AND member.kind = 'property'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner, ("Box".to_string(), "class".to_string()));
    write(
        &repo.root,
        "sample.py",
        "class Box:\n    class_value: int = 1\n\n    def __init__(self):\n        pass\n\n    def read(self):\n        return self.value + self.class_value\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Box");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "sample.py"
                && site.line == 8
                && site.caller.ends_with("read")),
        "missing dynamic attribute read in {change:?}"
    );
}

#[test]
fn a_removed_callable_class_attribute_reports_its_member_invocation() {
    let repo = committed_repo(&[(
        "sample.py",
        "def Factory(environ):\n    return {}\n\nclass App:\n    request_class = Factory\n\n    def make(self, environ):\n        return self.request_class(environ)\n\n    def write(self, replacement):\n        self.request_class = replacement\n\nclass Other:\n    request_class = Factory\n\n    def make_other(self, environ):\n        return self.request_class(environ)\n\nOther.request_class = Factory\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let member: (String, String) = conn
        .query_row(
            "SELECT member.kind, owner.name FROM symbols member
             JOIN symbols owner ON owner.symbol_id = member.parent_symbol_id
             WHERE member.path = 'sample.py' AND member.name = 'request_class'
               AND owner.name = 'App'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(member, ("variable".to_string(), "App".to_string()));
    let site: (String, Option<String>) = conn
        .query_row(
            "SELECT kind, json_extract(metadata_json, '$.receiver') FROM identifiers
             WHERE path = 'sample.py' AND name = 'request_class' AND start_line = 8",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(site, ("call".to_string(), Some("self".to_string())));
    write(
        &repo.root,
        "sample.py",
        "def Factory(environ):\n    return {}\n\nclass App:\n    def make(self, environ):\n        return self.request_class(environ)\n\n    def write(self, replacement):\n        self.request_class = replacement\n\nclass Other:\n    request_class = Factory\n\n    def make_other(self, environ):\n        return self.request_class(environ)\n\nOther.request_class = Factory\n",
    );

    let check = check(&repo);
    let change = changed(&check, "App");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "sample.py" && site.line == 6 && site.caller == "make"),
        "missing removed callable class-attribute invocation in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "sample.py" || ![9, 15, 17].contains(&site.line)),
        "reported a write or same-named other-owner invocation: {change:?}"
    );
}

#[test]
fn a_same_line_write_comment_or_string_does_not_hide_a_dynamic_member_read() {
    let repo = committed_repo(&[(
        "sample.py",
        "class Box:\n    def __init__(self):\n        self.deleted = 1\n    def read(self, other):\n        value = self.deleted; other.deleted = 1\n        text = \"other.deleted = 1\"; value = self.deleted\n        commented = self.deleted  # other.deleted = 1\n        other.deleted = 1\n        return value\n",
    )]);
    write(
        &repo.root,
        "sample.py",
        "class Box:\n    def __init__(self):\n        pass\n    def read(self, other):\n        value = self.deleted; other.deleted = 1\n        text = \"other.deleted = 1\"; value = self.deleted\n        commented = self.deleted  # other.deleted = 1\n        other.deleted = 1\n        return value\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Box");

    for line in [5, 6, 7] {
        assert!(
            change
                .proven
                .iter()
                .chain(&change.likely)
                .any(|site| site.path == "sample.py" && site.line == line),
            "missing dynamic read at sample.py:{line} in {change:?}"
        );
    }
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "sample.py" || site.line != 8),
        "reported a dynamic member write as a broken read: {change:?}"
    );
}

#[test]
fn a_css_custom_property_reference_is_a_site_in_the_reference_report() {
    let repo = committed_repo(&[(
        "sample.css",
        ":root { --accent-color: #ffffff; }\n.button { color: var(--accent-color); }\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "--accent-color", Some("sample.css")).unwrap();

    let report = reference_report(&conn, &repo.root, &target, false).unwrap();

    assert!(
        report.sites.iter().any(|site| {
            site.site.path == "sample.css"
                && site.site.start_line == Some(2)
                && site.site.kind == "references"
                && site.label == code_kb_core::SiteLabel::Proven
        }),
        "missing CSS custom-property reference in {report:?}"
    );
}

#[test]
fn removing_a_css_custom_property_reports_its_reference() {
    let repo = committed_repo(&[(
        "sample.css",
        ":root { --accent-color: #ffffff; }\n.button { color: var(--accent-color); }\n",
    )]);
    write(
        &repo.root,
        "sample.css",
        ":root { }\n.button { color: var(--accent-color); }\n",
    );

    let check = check(&repo);
    let change = check
        .changes
        .iter()
        .find(|change| change.display.ends_with("--accent-color"))
        .unwrap_or_else(|| panic!("missing custom-property change in {check:?}"));

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "sample.css" && site.line == 2),
        "missing CSS custom-property consumer in {change:?}"
    );
}

#[test]
fn another_css_custom_property_definition_does_not_inherit_a_base_proof() {
    let repo = committed_repo(&[(
        "sample.css",
        ":root { --accent-color: #ffffff; } .theme { color: black; }\n.button { color: var(--accent-color); }\n",
    )]);
    write(
        &repo.root,
        "sample.css",
        ":root { } .theme { --accent-color: #000000; }\n.button { color: var(--accent-color); }\n",
    );

    let check = check(&repo);
    let change = check
        .changes
        .iter()
        .find(|change| change.display.ends_with("--accent-color"))
        .unwrap_or_else(|| panic!("missing removed custom-property change in {check:?}"));

    assert!(
        change
            .proven
            .iter()
            .all(|site| site.path != "sample.css" || site.line != 2),
        "a base-only relationship proved a use despite a current same-name definition: {change:?}"
    );
}

#[test]
fn a_css_selector_composes_reference_is_a_site_in_the_reference_report() {
    let repo = committed_repo(&[(
        "sample.css",
        ".button { color: red; }\n.consumer { composes: button; }\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, ".button", Some("sample.css")).unwrap();

    let report = reference_report(&conn, &repo.root, &target, false).unwrap();

    assert!(
        report.sites.iter().any(|site| {
            site.site.path == "sample.css"
                && site.site.start_line == Some(2)
                && site.site.kind == "references"
                && site.label == code_kb_core::SiteLabel::Proven
        }),
        "missing CSS selector composition reference in {report:?}"
    );
}

#[test]
fn removing_a_css_selector_reports_its_composition_reference() {
    let repo = committed_repo(&[(
        "sample.css",
        ".button { color: red; }\n.consumer { composes: button; }\n",
    )]);
    write(
        &repo.root,
        "sample.css",
        ".consumer { composes: button; }\n",
    );

    let check = check(&repo);
    let change = changed(&check, ".button");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "sample.css" && site.line == 2),
        "missing CSS selector consumer in {change:?}"
    );
}

#[test]
fn a_resolved_call_inside_html_is_a_reference_site() {
    let repo = committed_repo(&[(
        "sample.html",
        "<script>\nfunction anchor_html(value) { return value + 41; }\nfunction caller_html() { return anchor_html(1); }\n</script>\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "anchor_html", Some("sample.html")).unwrap();

    let report = reference_report(&conn, &repo.root, &target, false).unwrap();

    assert!(
        report.sites.iter().any(|site| {
            site.site.path == "sample.html"
                && site.site.start_line == Some(3)
                && site.site.from_symbol_name == "caller_html"
                && site.site.kind == "calls"
                && site.label == code_kb_core::SiteLabel::Proven
        }),
        "missing embedded HTML call in {report:?}"
    );
}

#[test]
fn a_removed_javascript_member_reports_a_typed_receiver_read() {
    let repo = committed_repo(&[
        ("box.ts", "export class Box {\n  value: number = 1;\n}\n"),
        (
            "use.ts",
            "import { Box } from './box';\nexport function read(box: Box) { return box.value; }\n",
        ),
    ]);
    write(&repo.root, "box.ts", "export class Box {}\n");

    let check = check(&repo);
    let change = changed(&check, "Box");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "use.ts" && site.caller == "read"),
        "missing removed TypeScript field read in {change:?}"
    );
}

#[test]
fn removing_a_csharp_class_constant_reports_its_member_read() {
    let repo = committed_repo(&[(
        "sample.cs",
        "public class Limits { public const int Max = 10; public int Keep = 0; }\npublic class Service { public int Read() { return Limits.Max; } }\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let owner: (String, String) = conn
        .query_row(
            "SELECT parent.name, parent.kind FROM symbols member
             JOIN symbols parent ON parent.symbol_id = member.parent_symbol_id
             WHERE member.path = 'sample.cs' AND member.name = 'Max' AND member.kind = 'constant'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(owner, ("Limits".to_string(), "class".to_string()));
    write(
        &repo.root,
        "sample.cs",
        "public class Limits { public int Keep = 0; }\npublic class Service { public int Read() { return Limits.Max; } }\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Limits");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "sample.cs" && site.caller == "Read"),
        "missing class-constant read in {change:?}"
    );
}

#[test]
fn a_changed_callable_reports_a_value_dependency_without_proving_a_broken_call() {
    let repo = committed_repo(&[(
        "shapes.py",
        "def area(width):\n    return width\n\nhandlers = [area]\n",
    )]);
    write(
        &repo.root,
        "shapes.py",
        "def area(width, height):\n    return width\n\nhandlers = [area]\n",
    );

    let check = check(&repo);
    let change = changed(&check, "area");

    assert!(
        change.likely.iter().any(|site| {
            site.path == "shapes.py" && site.note.as_deref() == Some("callable value dependency")
        }),
        "missing advisory callback dependency in {change:?}"
    );
    assert!(
        change.proven.is_empty(),
        "a callback value reference was reported as a proven broken invocation: {change:?}"
    );
    assert!(
        stop_sites(&check).is_empty(),
        "a callback value reference created a proven Stop block: {check:?}"
    );
}

#[test]
fn a_changed_method_reports_only_its_same_receiver_member_value_dependency() {
    let repo = committed_repo(&[(
        "sample.py",
        "class Other:\n    def ensure_sync(self, func):\n        return func\n\nclass App:\n    def ensure_sync(self, func, **options):\n        return func\n\n    def install(self, *, _async_wrapper):\n        return _async_wrapper\n\n    def register(self, other: Other):\n        value = self.install(_async_wrapper=self.ensure_sync)\n        result = self.ensure_sync(other, mode=\"default\")\n        return self.install(_async_wrapper=other.ensure_sync)\n        self.ensure_sync = other.ensure_sync\n        possible = unknown.ensure_sync\n",
    )]);
    write(
        &repo.root,
        "sample.py",
        "class Other:\n    def ensure_sync(self, func):\n        return func\n\nclass App:\n    def ensure_sync(self, func, *, mode, **options):\n        return func\n\n    def install(self, *, _async_wrapper):\n        return _async_wrapper\n\n    def register(self, other: Other):\n        value = self.install(_async_wrapper=self.ensure_sync)\n        result = self.ensure_sync(other, mode=\"default\")\n        return self.install(_async_wrapper=other.ensure_sync)\n        self.ensure_sync = other.ensure_sync\n        possible = unknown.ensure_sync\n",
    );

    let check = check(&repo);
    let change = check
        .changes
        .iter()
        .find(|change| change.display == "App.ensure_sync")
        .unwrap_or_else(|| panic!("missing App.ensure_sync change in {check:?}"));

    assert!(
        change.likely.iter().any(|site| {
            site.path == "sample.py"
                && site.line == 13
                && site.note.as_deref() == Some("callable value dependency")
        }),
        "missing advisory member callback in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "sample.py" || ![14, 15, 16].contains(&site.line)),
        "claimed a same-name method on another receiver: {change:?}"
    );
    assert_eq!(change.unresolved, 1, "{change:?}");
}

#[test]
fn a_merged_call_site_reports_a_stale_call_after_a_valid_same_line_call() {
    let repo = committed_repo(&[(
        "Lib.kt",
        "fun compute(x: Int = 0): Int = x\nfun caller() { compute(x = 1, y = 2); compute(x = 1) }\n",
    )]);
    write(
        &repo.root,
        "Lib.kt",
        "fun compute(x: Int = 0, y: Int): Int = x + y\nfun caller() { compute(x = 1, y = 2); compute(x = 1) }\n",
    );

    let check = check(&repo);
    let change = changed(&check, "compute");

    assert!(
        change.proven.iter().chain(&change.likely).any(|site| {
            site.path == "Lib.kt"
                && site.line == 2
                && site.caller == "caller"
                && site.note.as_deref() == Some("missing required argument: y")
        }),
        "missing stale same-line call after a valid call: {change:?}"
    );
}

#[test]
fn a_same_line_call_on_another_typed_receiver_does_not_inherit_required_arguments() {
    let repo = committed_repo(&[(
        "Lib.kt",
        "class App {\n    fun compute(x: Int = 0): Int = x\n}\nclass Other {\n    fun compute(x: Int = 0): Int = x\n}\nfun use(app: App, other: Other) {\n    app.compute(x = 1, y = 2); other.compute(x = 1)\n}\n",
    )]);
    write(
        &repo.root,
        "Lib.kt",
        "class App {\n    fun compute(x: Int = 0, y: Int): Int = x + y\n}\nclass Other {\n    fun compute(x: Int = 0): Int = x\n}\nfun use(app: App, other: Other) {\n    app.compute(x = 1, y = 2); other.compute(x = 1)\n}\n",
    );

    let check = check(&repo);
    let change = changed(&check, "App.compute");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "Lib.kt" || site.line != 8),
        "a foreign receiver call was checked against App.compute: {change:?}"
    );
    assert_eq!(change.unresolved, 0, "{change:?}");
}

#[test]
fn removing_a_callable_value_preserves_its_proven_stale_binding() {
    let repo = committed_repo(&[(
        "shapes.py",
        "def before():\n    return 0\n\ndef area(width):\n    return width\n\nkeep = 1\nhandlers = [area]\n",
    )]);
    write(
        &repo.root,
        "shapes.py",
        "def before():\n    return 0\n\nkeep = 1\nhandlers = [area]\n",
    );

    let check = check(&repo);
    let change = changed(&check, "area");

    assert!(
        change
            .proven
            .iter()
            .any(|site| site.path == "shapes.py" && site.caller == "handlers"),
        "missing proven removed-callable binding in {change:?}"
    );
    assert!(
        stop_sites(&check)
            .iter()
            .any(|site| site.path == "shapes.py" && site.caller == "handlers"),
        "a proven stale value binding did not block: {check:?}"
    );
}

#[test]
fn removing_a_callable_member_keeps_a_read_before_a_write_and_omits_writes() {
    let repo = committed_repo(&[(
        "sample.py",
        "class App:\n    def ensure_sync(self, func):\n        return func\n\n\ndef read_and_write(app: App, replacement):\n    value = app.ensure_sync; app.ensure_sync = replacement\n\ndef write(app: App, replacement):\n    app.ensure_sync = replacement\n",
    )]);
    write(
        &repo.root,
        "sample.py",
        "class App:\n    pass\n\ndef read_and_write(app: App, replacement):\n    value = app.ensure_sync; app.ensure_sync = replacement\n\ndef write(app: App, replacement):\n    app.ensure_sync = replacement\n",
    );

    let check = check(&repo);
    let change = check
        .changes
        .iter()
        .find(|change| change.display == "App.ensure_sync")
        .unwrap_or_else(|| panic!("missing App.ensure_sync removal in {check:?}"));

    assert!(
        change
            .proven
            .iter()
            .any(|site| site.path == "sample.py" && site.line == 5),
        "missing removed method read before a same-line write in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "sample.py" || site.line != 8),
        "reported a removed method write as a stale read: {change:?}"
    );
}

#[test]
fn an_unresolved_callable_value_keeps_its_competing_definition_evidence() {
    let repo = committed_repo(&[
        ("first.py", "def area(width):\n    return width\n"),
        ("second.py", "def area(width):\n    return width * 2\n"),
        ("app.py", "handlers = [area]\n"),
    ]);
    write(
        &repo.root,
        "first.py",
        "def area(width, height=1):\n    return width\n",
    );

    let check = check(&repo);
    let change = changed(&check, "area");

    assert!(
        change.unresolved > 0,
        "competing callable definitions lost unresolved evidence: {change:?}"
    );
    assert!(
        change.likely.iter().all(|site| site.path != "app.py"),
        "an unresolved callback dependency became likely: {change:?}"
    );
}

#[test]
fn a_defaulted_powershell_parameter_does_not_turn_an_unresolved_range_into_a_stop() {
    let repo = committed_repo(&[(
        "commands.ps1",
        "function Send-Message { param([string]$Old) }\nfunction Caller { Send-Message -Old hello }\n",
    )]);
    write(
        &repo.root,
        "commands.ps1",
        "function Send-Message { param([string]$Old, [string]$Encoding = 'utf8') }\nfunction Caller { Send-Message -Old hello }\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Send-Message");

    assert!(
        change.likely.iter().any(|site| site.caller == "Caller"),
        "missing advisory call-site dependency in {change:?}"
    );
    assert!(
        stop_sites(&check).is_empty(),
        "unknown PowerShell arity was used as a proven Stop block: {check:?}"
    );
}

#[test]
fn csharp_qualified_constructor_calls_use_their_namespace_without_using_directives() {
    let repo = committed_repo(&[
        ("A.cs", "namespace A;\npublic record Point(int Y);\n"),
        ("B.cs", "namespace B;\npublic record Point(int X = 0);\n"),
        ("CA.cs", "namespace C.A;\npublic record Point(int Z = 0);\n"),
        (
            "Outer.cs",
            "public class Outer { public class Point { public Point() {} } }\n",
        ),
        (
            "Use.cs",
            "public class Use { public static void Make() {\n    _ = new A.Point(Y: 2);\n    _ = new B.Point(X: 1);\n    _ = new Point(Y: 3);\n} }\n",
        ),
        (
            "NestedUse.cs",
            "public class NestedUse { public static void Make() {\n    _ = new Outer.Point();\n} }\n",
        ),
        (
            "RelativeUse.cs",
            "namespace C;\npublic class RelativeUse { public static void Make() {\n    _ = new A.Point(Z: 1);\n} }\n",
        ),
        (
            "Alias.cs",
            "namespace C { using A = global::B; public class AliasUse { public static void Make() { _ = new A.Point(X: 1); } } }\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    let a = select_reference_target(&conn, "Point", Some("A.cs")).unwrap();
    let b = select_reference_target(&conn, "Point", Some("B.cs")).unwrap();
    let c_a = select_reference_target(&conn, "Point", Some("CA.cs")).unwrap();
    let nested = select_reference_target(&conn, "Point", Some("Outer.cs")).unwrap();
    let a_report = reference_report(&conn, &repo.root, &a, false).unwrap();
    let b_report = reference_report(&conn, &repo.root, &b, false).unwrap();
    let c_a_report = reference_report(&conn, &repo.root, &c_a, false).unwrap();
    let nested_report = reference_report(&conn, &repo.root, &nested, false).unwrap();

    assert!(
        a_report.sites.iter().any(|site| {
            site.site.path == "Use.cs"
                && site.site.start_line == Some(2)
                && site.label == SiteLabel::Proven
        }),
        "qualified A.Point call was not proven: {a_report:?}"
    );
    assert!(
        a_report.sites.iter().any(|site| {
            site.site.path == "Use.cs"
                && site.site.start_line == Some(3)
                && site.label == SiteLabel::Excluded
        }),
        "qualified B.Point call was not excluded from A.Point: {a_report:?}"
    );
    assert!(
        a_report.sites.iter().any(|site| {
            site.site.path == "Use.cs"
                && site.site.start_line == Some(4)
                && site.label == SiteLabel::Excluded
                && site.basis == "namespace `A` is not open here"
        }),
        "bare Point call no longer respects unopened namespace exclusion: {a_report:?}"
    );
    assert!(
        a_report.sites.iter().any(|site| {
            site.site.path == "RelativeUse.cs"
                && site.site.start_line == Some(3)
                && site.label == SiteLabel::Unresolved
        }),
        "relative A.Point was proven against global A.Point inside namespace C: {a_report:?}"
    );
    assert!(
        c_a_report.sites.iter().any(|site| {
            site.site.path == "RelativeUse.cs"
                && site.site.start_line == Some(3)
                && site.label == SiteLabel::Unresolved
        }),
        "relative A.Point was excluded from C.A.Point inside namespace C: {c_a_report:?}"
    );
    assert!(
        nested_report.sites.iter().any(|site| {
            site.site.path == "NestedUse.cs"
                && site.site.start_line == Some(2)
                && site.label == SiteLabel::Proven
        }),
        "Outer.Point was not bound through its extracted type owner: {nested_report:?}"
    );
    assert!(
        b_report.sites.iter().any(|site| {
            site.site.path == "Use.cs"
                && site.site.start_line == Some(3)
                && site.label == SiteLabel::Proven
        }),
        "qualified B.Point call was not proven: {b_report:?}"
    );
    assert!(
        a_report
            .sites
            .iter()
            .any(|site| { site.site.path == "Alias.cs" && site.label == SiteLabel::Unresolved }),
        "an indexed namespace alias was treated as a literal namespace: {a_report:?}"
    );
}

#[test]
fn csharp_record_field_removal_keeps_relative_namespace_calls_unresolved() {
    let repo = committed_repo(&[
        (
            "A.cs",
            "namespace A;\npublic record Point(int Y, int X = 0);\n",
        ),
        ("CA.cs", "namespace C.A;\npublic record Point(int X = 0);\n"),
        (
            "RelativeUse.cs",
            "namespace C;\npublic class RelativeUse { public static void Make() {\n    _ = new A.Point(X: 1);\n} }\n",
        ),
    ]);
    write(
        &repo.root,
        "A.cs",
        "namespace A;\npublic record Point(int Y);\n",
    );

    let check = check(&repo);
    let change = changed(&check, "A.Point");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| { site.path != "RelativeUse.cs" || site.line != 3 }),
        "relative C.A.Point was treated as a proven A.Point construction: {change:?}"
    );
    assert!(
        change.unresolved > 0,
        "relative C# type qualification lost its uncertainty: {change:?}"
    );
}

#[test]
fn csharp_record_field_removal_does_not_borrow_a_foreign_same_line_constructor_argument() {
    let repo = committed_repo(&[
        (
            "A.cs",
            "namespace A;\npublic record Point(int Y, int X = 0);\n",
        ),
        ("B.cs", "namespace B;\npublic record Point(int X = 0);\n"),
        (
            "Use.cs",
            "public class Use { public static void Make() {\n    _ = new A.Point(Y: 2); _ = new B.Point(X: 1);\n    _ = new A.Point(X: 1);\n} }\n",
        ),
    ]);
    write(
        &repo.root,
        "A.cs",
        "namespace A;\npublic record Point(int Y);\n",
    );

    let check = check(&repo);
    let change = changed(&check, "A.Point");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "Use.cs" && site.line == 3),
        "missing the A.Point call that still names removed X: {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "Use.cs" || site.line != 2),
        "borrowed B.Point's X argument for the A.Point call: {change:?}"
    );
}

#[test]
fn adding_a_dataclass_classvar_tracks_the_member_without_changing_construction() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\nfrom typing import ClassVar\n\n@dataclass\nclass Rec:\n    a: int\n",
        ),
        (
            "use.py",
            "from shapes import Rec\n\ndef make():\n    return Rec()\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\nfrom typing import ClassVar\n\n@dataclass\nclass Rec:\n    a: int\n    c: ClassVar[int]\n    plain = 2\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Rec");

    assert!(
        change.change.contains("+2")
            && change.change.contains("c")
            && change.change.contains("plain")
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "use.py" || site.line != 4),
        "a ClassVar changed dataclass constructor validity: {change:?}"
    );
}

#[test]
fn removing_dataclass_class_variables_reports_reads_without_blocking_construction() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\nfrom typing import ClassVar\n\n@dataclass\nclass Rec:\n    a: int\n    c: ClassVar[int] = 1\n    plain = 2\n",
        ),
        (
            "use.py",
            "from shapes import Rec\n\ndef use():\n    item = Rec()\n    c_value = Rec.c\n    return Rec.plain\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\nfrom typing import ClassVar\n\n@dataclass\nclass Rec:\n    a: int\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Rec");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "use.py" && site.line == 5),
        "missing removed ClassVar read in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "use.py" && site.line == 6),
        "missing removed unannotated class variable read in {change:?}"
    );
    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .all(|site| site.path != "use.py" || site.line != 4),
        "removing class variables changed dataclass constructor validity: {change:?}"
    );
}

#[test]
fn removing_a_dataclass_instance_field_still_reports_its_constructor() {
    let repo = committed_repo(&[
        (
            "shapes.py",
            "from dataclasses import dataclass\n\n@dataclass\nclass Rec:\n    a: int\n    c: int = 1\n",
        ),
        (
            "use.py",
            "from shapes import Rec\n\ndef make():\n    return Rec(c=1)\n",
        ),
    ]);
    write(
        &repo.root,
        "shapes.py",
        "from dataclasses import dataclass\n\n@dataclass\nclass Rec:\n    a: int\n",
    );

    let check = check(&repo);
    let change = changed(&check, "Rec");

    assert!(
        change
            .proven
            .iter()
            .chain(&change.likely)
            .any(|site| site.path == "use.py" && site.line == 4),
        "missing removed instance field constructor in {change:?}"
    );
}
