use code_kb_core::{
    LabeledSite, ReferenceReport, SiteLabel, Workspace, find_julie_extract_binary,
    find_references_scoped, open_read_only, reference_report, safe_tempdir, scan_workspace,
    select_reference_target,
};
use std::fs;
use std::path::PathBuf;

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

fn report(repo: &Repo, name: &str, path: Option<&str>) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, path).unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn dump(report: &ReferenceReport) -> String {
    let mut out = format!("{:?}\n", report.counts);
    for site in &report.sites {
        out.push_str(&format!(
            "{} {}:{}:{:?} end={:?} {} [{}] from={} kind={}\n",
            site.label.as_str(),
            site.site.path,
            site.site.start_line.unwrap_or(0),
            site.site.start_column,
            site.end_line,
            site.basis,
            site.site
                .occurrences
                .map_or(String::new(), |n| n.to_string()),
            site.site.from_symbol_name,
            site.site.kind,
        ));
    }
    out.push_str(&format!(
        "text_only={:?} cannot_see={:?} imports={:?}\n",
        report.text_only, report.cannot_see, report.imports
    ));
    out
}

fn line_of(repo: &Repo, path: &str, needle: &str) -> usize {
    let text = fs::read_to_string(repo.root.join(path)).unwrap();
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in {path}"))
        + 1
}

fn site_at<'a>(report: &'a ReferenceReport, path: &str, line: usize) -> &'a LabeledSite {
    report
        .sites
        .iter()
        .find(|site| site.site.path == path && site.site.start_line == Some(line))
        .unwrap_or_else(|| panic!("no site at {path}:{line}\n{}", dump(report)))
}

fn assert_site(report: &ReferenceReport, path: &str, line: usize, label: SiteLabel, basis: &str) {
    let site = site_at(report, path, line);
    assert_eq!(
        (site.label, site.basis.as_str()),
        (label, basis),
        "{path}:{line}\n{}",
        dump(report)
    );
}

const PY: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    (
        "pkg/models.py",
        "class Base:\n    def render(self):\n        return 1\n\n    def run(self):\n        return self.render()\n\n\nclass Child(Base):\n    def go(self):\n        return self.render()\n\n\nclass Config:\n    @staticmethod\n    def create():\n        return Config()\n\n    def load(self):\n        return 2\n\n\ndef make():\n    return Config()\n\n\ndef helper():\n    return 3\n\n\ndef use_helper():\n    return helper()\n",
    ),
    ("pkg/client.py", "def get():\n    return 1\n"),
    (
        "pkg/app.py",
        "import requests\nfrom pkg import models\nfrom pkg.models import Config\n\n\ndef typed(cfg: Config):\n    return cfg.load()\n\n\ndef untyped(cfg):\n    return cfg.load()\n\n\ndef builds():\n    cfg = Config()\n    return cfg.load()\n\n\ndef via_owner():\n    return Config.create()\n\n\ndef via_module():\n    return models.make()\n\n\ndef external():\n    return requests.load()\n\n\ndef fetch():\n    return requests.get()\n",
    ),
];

const PY_MULTI: &[(&str, &str)] = &[
    (
        "shapes/a.py",
        "def area():\n    return 1\n\n\ndef call_a():\n    return area()\n\n\ndef shadowed(area):\n    return area()\n\n\nhandlers = [area]\n\n\ndef perimeter():\n    return 5\n",
    ),
    (
        "shapes/b.py",
        "def area():\n    return 2\n\n\ndef call_b():\n    return area()\n",
    ),
    ("shapes/c.py", "def area():\n    return 3\n"),
    ("app/main.py", "def call_nothing():\n    return area()\n"),
    (
        "animals/zoo.py",
        "class Dog:\n    def speak(self):\n        return 1\n\n\nclass Cat:\n    def speak(self):\n        return 2\n\n\nclass Cow:\n    def speak(self):\n        return 3\n\n\ndef talk(d: Dog):\n    return d.speak()\n\n\ndef purr(c: Cat):\n    return c.speak()\n\n\ndef plain(x):\n    return x.speak()\n",
    ),
    (
        "web/page.js",
        "function perimeter() {\n  return 4;\n}\nfunction use() {\n  return perimeter();\n}\n",
    ),
];

const PY_TYPED_ELSEWHERE: &[(&str, &str)] = &[(
    "zoo/pets.py",
    "class Dog:\n    def fetch(self):\n        return 1\n\n\nclass Cat:\n    def purr(self):\n        return 2\n\n\ndef pet(c: Cat):\n    return c.purr()\n\n\ndef walk(d: Dog):\n    return d.purr()\n",
)];

const GO: &[(&str, &str)] = &[
    (
        "svc/a.go",
        "package svc\n\nfunc Helper() int {\n\treturn 1\n}\n",
    ),
    (
        "svc/b.go",
        "package svc\n\nfunc Use() int {\n\treturn Helper()\n}\n",
    ),
    (
        "other/c.go",
        "package other\n\nfunc Helper() int {\n\treturn 2\n}\n\nfunc Use2() int {\n\treturn Helper()\n}\n",
    ),
    (
        "svc/closure.go",
        "package svc\n\nfunc outere() int {\n\tinnere := func() int { return 1 }\n\treturn innere()\n}\n",
    ),
];

const JAVA: &[(&str, &str)] = &[
    (
        "com/a/Util.java",
        "package com.a;\n\npublic class Util {\n    public static int help() {\n        return 1;\n    }\n}\n",
    ),
    (
        "com/a/User.java",
        "package com.a;\n\npublic class User {\n    int go() {\n        Util u = null;\n        return Util.help();\n    }\n}\n",
    ),
];

const RS: &[(&str, &str)] = &[
    (
        "crates/a-core/src/lib.rs",
        "pub mod util;\n\npub fn safe_dir() -> u32 {\n    1\n}\n\npub fn read() -> u32 {\n    2\n}\n",
    ),
    (
        "crates/a-core/src/util.rs",
        "pub fn calls_crate() -> u32 {\n    crate::safe_dir()\n}\n\npub fn calls_std() -> usize {\n    std::fs::read(\"x\").map(|b| b.len()).unwrap_or(0)\n}\n",
    ),
    (
        "crates/a-cli/src/main.rs",
        "fn calls_crate_name() -> u32 {\n    a_core::safe_dir()\n}\n\nfn main() {}\n",
    ),
];

const JS: &[(&str, &str)] = &[
    (
        "src/builder.js",
        "class Builder {\n  with(x) {\n    return this;\n  }\n  finish() {\n    return 1;\n  }\n}\nmodule.exports = { Builder };\n",
    ),
    (
        "src/use.js",
        "const { Builder } = require('./builder');\n\nfunction build() {\n  const b = new Builder();\n  return b\n    .with(1)\n    .with(2)\n    .finish();\n}\n",
    ),
    (
        "src/view.js",
        "class View {\n  render() {\n    return 1;\n  }\n}\nfunction peek(app) {\n  return app.render;\n}\nfunction clamp(v) {\n  return Math.max(v, 0);\n}\nfunction max(a, b) {\n  return a > b ? a : b;\n}\n",
    ),
];

const CTOR_PY: &[(&str, &str)] = &[
    (
        "pkg/shapes.py",
        "class Circle:\n    def __init__(self, r):\n        self.r = r\n",
    ),
    (
        "pkg/use.py",
        "from pkg.shapes import Circle\n\n\ndef make():\n    return Circle(2)\n",
    ),
];

const CTOR_JAVA: &[(&str, &str)] = &[
    (
        "com/a/Widget.java",
        "package com.a;\n\npublic class Widget {\n    public Widget(int x) {\n    }\n\n    static Widget copy() {\n        return new Widget(2);\n    }\n}\n",
    ),
    (
        "com/a/Maker.java",
        "package com.a;\n\npublic class Maker {\n    Widget make() {\n        return new Widget(1);\n    }\n}\n",
    ),
];

const FIXTURE_BUDGET_OTHERS: usize = 13;

fn budget_fixture(definitions: usize) -> Vec<(String, String)> {
    let mut files = Vec::new();
    for n in 1..=definitions {
        files.push((
            format!("pkg/m{n:02}.py"),
            format!("class C{n:02}:\n    def run(self):\n        return {n}\n"),
        ));
        files.push((
            format!("pkg/use{n:02}.py"),
            format!(
                "from pkg.m{n:02} import C{n:02}\n\n\ndef call{n:02}():\n    o = C{n:02}()\n    return o.run()\n"
            ),
        ));
    }
    files
}

fn scanned_owned(files: &[(String, String)]) -> Repo {
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    scanned_repo(&borrowed)
}

fn assert_counts_add_up(report: &ReferenceReport) {
    let counts = &report.counts;
    assert_eq!(
        counts.text_lines,
        counts.site_lines + counts.definition_lines + counts.import_lines + counts.text_only_lines,
        "{}",
        dump(report)
    );
    assert_eq!(
        counts.sites,
        counts.proven + counts.likely + counts.unresolved + counts.excluded
    );
    assert_eq!(counts.sites, report.sites.len());
}

#[test]
fn a_call_in_the_same_file_is_proven_by_file() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "helper", None);

    let line = line_of(&repo, "pkg/models.py", "return helper()");
    assert_site(
        &report,
        "pkg/models.py",
        line,
        SiteLabel::Proven,
        "same file",
    );
}

#[test]
fn a_receiver_named_for_the_owner_is_proven_by_qualified_path() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "create", None);

    let line = line_of(&repo, "pkg/app.py", "Config.create()");
    assert_site(
        &report,
        "pkg/app.py",
        line,
        SiteLabel::Proven,
        "qualified path",
    );
}

#[test]
fn a_receiver_with_a_type_fact_for_the_owner_is_proven_by_receiver_type() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "load", None);

    let line = line_of(&repo, "pkg/app.py", "return cfg.load()");
    assert_site(
        &report,
        "pkg/app.py",
        line,
        SiteLabel::Proven,
        "receiver type",
    );
}

#[test]
fn a_self_call_to_an_inherited_method_is_proven_by_class_chain() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "Base.render", None);

    let line = line_of(&repo, "pkg/models.py", "def go(self)") + 1;
    assert_site(
        &report,
        "pkg/models.py",
        line,
        SiteLabel::Proven,
        "class chain",
    );
}

#[test]
fn a_receiver_bound_to_the_defining_module_is_proven_by_import() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "make", None);

    let line = line_of(&repo, "pkg/app.py", "models.make()");
    assert_site(&report, "pkg/app.py", line, SiteLabel::Proven, "import");
}

#[test]
fn a_receiver_assigned_from_the_owner_constructor_is_proven_by_the_class_it_builds() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "load", None);

    let builds = line_of(&repo, "pkg/app.py", "def builds()");
    assert_site(
        &report,
        "pkg/app.py",
        builds + 2,
        SiteLabel::Proven,
        "builds the class",
    );
}

#[test]
fn an_untyped_receiver_of_the_only_definition_is_likely_by_name_only() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "load", None);

    let untyped = line_of(&repo, "pkg/app.py", "def untyped(cfg)");
    assert_site(
        &report,
        "pkg/app.py",
        untyped + 1,
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_bare_call_that_several_files_could_define_is_likely_the_nearest_of_k() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    assert_site(
        &report,
        "app/main.py",
        2,
        SiteLabel::Likely,
        "nearest of 3 definitions",
    );
}

#[test]
fn an_untyped_receiver_with_three_definitions_is_unresolved() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "Cat.speak", None);

    let line = line_of(&repo, "animals/zoo.py", "def plain(x)") + 1;
    assert_site(
        &report,
        "animals/zoo.py",
        line,
        SiteLabel::Unresolved,
        "3 definitions",
    );
}

#[test]
fn a_parameter_that_shadows_the_function_excludes_the_call() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    let line = line_of(&repo, "shapes/a.py", "def shadowed(area)");
    let site = site_at(&report, "shapes/a.py", line + 1);
    assert_eq!(site.label, SiteLabel::Excluded, "{}", dump(&report));
    assert_eq!(
        site.basis,
        format!("resolves to `area` (shapes/a.py:{line})"),
        "{}",
        dump(&report)
    );
}

#[test]
fn a_variable_reference_to_the_function_in_its_file_is_proven() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    let line = line_of(&repo, "shapes/a.py", "handlers = [area]");
    let site = site_at(&report, "shapes/a.py", line);
    assert_eq!(
        (site.label, site.basis.as_str(), site.site.kind.as_str()),
        (SiteLabel::Proven, "same file", "variable_ref")
    );
}

#[test]
fn a_site_typed_to_another_definition_is_excluded_with_resolves_to() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "Cat.speak", None);

    let dog = line_of(&repo, "animals/zoo.py", "class Dog") + 1;
    let line = line_of(&repo, "animals/zoo.py", "def talk(d: Dog)") + 1;
    assert_site(
        &report,
        "animals/zoo.py",
        line,
        SiteLabel::Excluded,
        &format!("resolves to `Dog.speak` (animals/zoo.py:{dog})"),
    );
}

#[test]
fn a_call_that_julie_resolved_to_another_definition_is_excluded_with_resolves_to() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    assert_site(
        &report,
        "shapes/b.py",
        6,
        SiteLabel::Excluded,
        "resolves to `area` (shapes/b.py:1)",
    );
}

#[test]
fn a_receiver_imported_from_an_unindexed_module_is_excluded() {
    let repo = scanned_repo(PY);

    let method = report(&repo, "load", None);
    let line = line_of(&repo, "pkg/app.py", "requests.load()");
    assert_site(
        &method,
        "pkg/app.py",
        line,
        SiteLabel::Excluded,
        "receiver `requests` is imported from `requests`",
    );

    let function = report(&repo, "get", None);
    let line = line_of(&repo, "pkg/app.py", "requests.get()");
    assert_site(
        &function,
        "pkg/app.py",
        line,
        SiteLabel::Excluded,
        "receiver `requests` is imported from `requests`",
    );
}

#[test]
fn a_receiver_typed_to_an_unrelated_class_is_excluded_by_its_type() {
    let repo = scanned_repo(PY_TYPED_ELSEWHERE);
    let report = report(&repo, "Cat.purr", None);

    let line = line_of(&repo, "zoo/pets.py", "return d.purr()");
    assert_site(
        &report,
        "zoo/pets.py",
        line,
        SiteLabel::Excluded,
        "receiver `d` has type `Dog`",
    );
}

#[test]
fn a_capitalized_receiver_that_no_definition_names_is_excluded_as_a_library_type() {
    let repo = scanned_repo(JS);
    let report = report(&repo, "max", None);

    let line = line_of(&repo, "src/view.js", "Math.max");
    assert_site(
        &report,
        "src/view.js",
        line,
        SiteLabel::Excluded,
        "receiver `Math` has type `Math`",
    );
}

#[test]
fn a_call_in_another_language_family_is_excluded() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "perimeter", Some("shapes/a.py"));

    let line = line_of(&repo, "web/page.js", "return perimeter()");
    assert_site(
        &report,
        "web/page.js",
        line,
        SiteLabel::Excluded,
        "other language",
    );
}

#[test]
fn a_bare_go_call_in_the_same_folder_is_proven_by_package() {
    let repo = scanned_repo(GO);
    let report = report(&repo, "Helper", Some("svc/a.go"));

    let line = line_of(&repo, "svc/b.go", "return Helper()");
    assert_site(&report, "svc/b.go", line, SiteLabel::Proven, "same package");
    let other = line_of(&repo, "other/c.go", "return Helper()");
    assert_eq!(
        site_at(&report, "other/c.go", other).label,
        SiteLabel::Excluded
    );
}

#[test]
fn a_java_type_in_the_same_folder_is_proven_by_package() {
    let repo = scanned_repo(JAVA);
    let report = report(&repo, "Util", None);

    let line = line_of(&repo, "com/a/User.java", "Util u = null");
    assert_site(
        &report,
        "com/a/User.java",
        line,
        SiteLabel::Proven,
        "same package",
    );
}

#[test]
fn a_local_function_called_by_its_enclosing_function_is_proven_by_enclosing_scope() {
    let repo = scanned_repo(GO);
    let report = report(&repo, "innere", None);

    let line = line_of(&repo, "svc/closure.go", "return innere()");
    assert_site(
        &report,
        "svc/closure.go",
        line,
        SiteLabel::Proven,
        "enclosing scope",
    );
}

#[test]
fn a_member_access_the_resolver_cannot_type_is_likely_with_receiver_not_typed() {
    let repo = scanned_repo(JS);
    let report = report(&repo, "render", None);

    let line = line_of(&repo, "src/view.js", "return app.render");
    assert_site(
        &report,
        "src/view.js",
        line,
        SiteLabel::Likely,
        "receiver not typed",
    );
}

#[test]
fn rust_path_calls_are_proven_by_qualified_path_and_std_paths_are_excluded() {
    let repo = scanned_repo(RS);

    let safe_dir = report(&repo, "safe_dir", None);
    let by_crate = line_of(&repo, "crates/a-core/src/util.rs", "crate::safe_dir()");
    assert_site(
        &safe_dir,
        "crates/a-core/src/util.rs",
        by_crate,
        SiteLabel::Proven,
        "qualified path",
    );
    let by_name = line_of(&repo, "crates/a-cli/src/main.rs", "a_core::safe_dir()");
    assert_site(
        &safe_dir,
        "crates/a-cli/src/main.rs",
        by_name,
        SiteLabel::Proven,
        "qualified path",
    );

    let read = report(&repo, "read", None);
    let std_call = line_of(&repo, "crates/a-core/src/util.rs", "std::fs::read");
    assert_eq!(
        site_at(&read, "crates/a-core/src/util.rs", std_call).label,
        SiteLabel::Excluded
    );
}

#[test]
fn every_name_site_appears_once_with_one_label() {
    let repo = scanned_repo(PY_MULTI);
    let conn = open_read_only(&repo.db).unwrap();
    let mut expected: Vec<(String, i64)> = Vec::new();
    for sql in [
        "SELECT replace(path, '\\', '/'), start_line FROM pending_relationships
         WHERE target_terminal_name = 'area' AND kind != 'imports'",
        "SELECT replace(i.path, '\\', '/'), i.start_line FROM identifiers i
         WHERE i.name = 'area' AND i.language = 'python'
           AND NOT EXISTS (SELECT 1 FROM symbols d WHERE d.name = 'area' AND d.path = i.path
                           AND d.start_line = i.start_line)",
        "SELECT replace(r.path, '\\', '/'), r.start_line FROM relationships r
         JOIN symbols t ON t.symbol_id = r.to_symbol_id WHERE t.name = 'area'",
    ] {
        let mut stmt = conn.prepare(sql).unwrap();
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .unwrap();
        expected.extend(rows.map(Result::unwrap));
    }
    expected.sort();
    expected.dedup();
    expected.retain(|(path, _)| path.ends_with(".py"));

    for path in ["shapes/a.py", "shapes/b.py", "shapes/c.py"] {
        let report = report(&repo, "area", Some(path));
        let mut seen: Vec<(String, i64)> = report
            .sites
            .iter()
            .filter(|site| site.site.path.ends_with(".py"))
            .map(|site| (site.site.path.clone(), site.site.start_line.unwrap() as i64))
            .collect();
        seen.sort();
        assert_eq!(seen, expected, "{path}\n{}", dump(&report));
        assert_counts_add_up(&report);
    }
}

#[test]
fn a_multi_line_chained_call_reports_the_line_of_the_method_name() {
    let repo = scanned_repo(JS);
    let report = report(&repo, "finish", None);

    let line = line_of(&repo, "src/use.js", ".finish()");
    assert_eq!(report.sites.len(), 1, "{}", dump(&report));
    let site = site_at(&report, "src/use.js", line);
    assert_eq!(site.end_line, None);
    assert_eq!(report.counts.site_lines, 1);
    assert_eq!(report.counts.text_only_lines, 0);
    assert!(report.text_only.is_empty());
}

#[test]
fn chained_calls_to_one_method_keep_one_site_for_each_line() {
    let repo = scanned_repo(JS);
    let report = report(&repo, "with", None);

    let first = line_of(&repo, "src/use.js", ".with(1)");
    let lines: Vec<usize> = report
        .sites
        .iter()
        .map(|site| site.site.start_line.unwrap())
        .collect();
    assert_eq!(lines, vec![first, first + 1], "{}", dump(&report));
    assert_eq!(report.counts.text_only_lines, 0);
}

#[test]
fn definition_import_and_comment_lines_are_counted_apart() {
    let repo = scanned_repo(&[
        ("pkg/models.py", "def helper():\n    return 3\n"),
        (
            "pkg/app.py",
            "from pkg.models import helper\n\n# helper is called below\n\n\ndef run():\n    return helper()\n",
        ),
        ("README.md", "Call `helper` to start.\n"),
    ]);
    let report = report(&repo, "helper", None);

    assert_eq!(report.counts.text_lines, 5, "{}", dump(&report));
    assert_eq!(report.counts.site_lines, 1);
    assert_eq!(report.counts.definition_lines, 1);
    assert_eq!(report.counts.import_lines, 1);
    assert_eq!(report.counts.text_only_lines, 2);
    assert_counts_add_up(&report);
    let text_only: Vec<(&str, &[usize])> = report
        .text_only
        .iter()
        .map(|entry| (entry.path.as_str(), entry.lines.as_slice()))
        .collect();
    assert_eq!(
        text_only,
        vec![("README.md", &[1usize][..]), ("pkg/app.py", &[3usize][..])]
    );
    assert!(report.text_scan.complete);
    assert_eq!(report.text_scan.skipped, None);
    assert!(report.text_scan.files_scanned > 0);
}

#[test]
fn a_constructor_target_skips_the_text_scan_and_counts_class_calls() {
    let repo = scanned_repo(CTOR_PY);
    let report = report(&repo, "Circle.__init__", None);

    assert_eq!(report.text_scan.skipped.as_deref(), Some("Circle"));
    assert_eq!(report.text_scan.files_scanned, 0);
    assert_eq!(report.counts.text_lines, 0);
    assert!(report.text_only.is_empty());
    let line = line_of(&repo, "pkg/use.py", "Circle(2)");
    assert_site(
        &report,
        "pkg/use.py",
        line,
        SiteLabel::Proven,
        "builds the class",
    );
}

#[test]
fn a_java_constructor_target_skips_the_text_scan_and_counts_new_expressions() {
    let repo = scanned_repo(CTOR_JAVA);
    let report = report(&repo, "Widget.Widget", None);

    assert_eq!(report.text_scan.skipped.as_deref(), Some("Widget"));
    let line = line_of(&repo, "com/a/Maker.java", "new Widget(1)");
    assert_site(
        &report,
        "com/a/Maker.java",
        line,
        SiteLabel::Proven,
        "builds the class",
    );
}

#[test]
fn at_most_twelve_other_definitions_are_checked_and_the_rest_are_noted() {
    let repo = scanned_owned(&budget_fixture(FIXTURE_BUDGET_OTHERS + 1));
    let report = report(&repo, "C01.run", None);

    let unchecked = "1 other definition with this name was not checked";
    assert!(
        report.cannot_see.iter().any(|note| note == unchecked),
        "{:?}",
        report.cannot_see
    );
    let line = line_of(&repo, "pkg/use14.py", "return o.run()");
    assert_site(
        &report,
        "pkg/use14.py",
        line,
        SiteLabel::Unresolved,
        "14 definitions",
    );
    let line = line_of(&repo, "pkg/use13.py", "return o.run()");
    assert_eq!(
        site_at(&report, "pkg/use13.py", line).label,
        SiteLabel::Excluded
    );
}

#[test]
fn twelve_other_definitions_are_all_checked_without_a_note() {
    let repo = scanned_owned(&budget_fixture(FIXTURE_BUDGET_OTHERS));
    let report = report(&repo, "C01.run", None);

    assert!(
        report
            .cannot_see
            .iter()
            .all(|note| !note.contains("not checked")),
        "{:?}",
        report.cannot_see
    );
    let line = line_of(&repo, "pkg/use13.py", "return o.run()");
    assert_eq!(
        site_at(&report, "pkg/use13.py", line).label,
        SiteLabel::Excluded
    );
}

#[test]
fn the_report_lists_what_the_index_cannot_see_for_the_language() {
    let repo = scanned_repo(PY);
    let report = report(&repo, "helper", None);

    assert_eq!(
        report.cannot_see,
        vec![
            "calls through getattr() or names built at runtime",
            "calls through dynamic dispatch reach overrides that this list does not show",
        ]
    );
    assert_eq!(report.target.display_name, "helper");
    assert_eq!(report.target.definitions_with_name, 1);
}

#[test]
fn the_target_names_its_owner_and_counts_definitions_with_the_name() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "Cat.speak", None);

    assert_eq!(report.target.display_name, "Cat.speak");
    assert_eq!(report.target.definitions_with_name, 3);
    assert_eq!(report.target.path, "animals/zoo.py");
}

#[test]
fn sites_are_sorted_by_label_then_path_line_and_column() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    let keys: Vec<(usize, &str, usize, usize)> = report
        .sites
        .iter()
        .map(|site| {
            (
                site.label.rank(),
                site.site.path.as_str(),
                site.site.start_line.unwrap_or(0),
                site.site.start_column.unwrap_or(0),
            )
        })
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
}

#[test]
fn selecting_a_missing_or_ambiguous_name_fails_like_find_references() {
    let repo = scanned_repo(PY_MULTI);
    let conn = open_read_only(&repo.db).unwrap();

    for (name, path) in [
        ("nonexistent_name", None),
        ("area", None),
        ("area", Some("zzz")),
    ] {
        let selected = select_reference_target(&conn, name, path).unwrap_err();
        let found = find_references_scoped(&conn, name, "callers", 20, false, path).unwrap_err();
        assert_eq!(selected.to_string(), found.to_string());
    }
}

const INHERITANCE: &[(&str, &str)] = &[
    ("k/base.py", "class Base:\n    pass\n"),
    (
        "k/child.py",
        "from k.base import Base\n\n\nclass Child(\n    Base,\n):\n    pass\n\n\nclass Other(Base):\n    pass\n",
    ),
];

#[test]
fn a_base_class_listed_on_a_later_header_line_reports_that_line() {
    let repo = scanned_repo(INHERITANCE);
    let report = report(&repo, "Base", None);

    let multi_line = line_of(&repo, "k/child.py", "    Base,");
    let one_line = line_of(&repo, "k/child.py", "class Other(Base)");
    assert_site(
        &report,
        "k/child.py",
        multi_line,
        SiteLabel::Proven,
        "import",
    );
    assert_site(&report, "k/child.py", one_line, SiteLabel::Proven, "import");
    assert_eq!(report.counts.import_lines, 1);
    assert_eq!(report.counts.text_only_lines, 0);
}

const RS_TYPES: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod caller;\n\npub struct Alpha;\n\nimpl Alpha {\n    pub fn new() -> Alpha {\n        Alpha\n    }\n}\n\npub struct Beta;\n\nimpl Beta {\n    pub fn new() -> Beta {\n        Beta\n    }\n}\n\npub struct Gamma;\n\npub fn simplified() -> u32 {\n    1\n}\n",
    ),
    (
        "src/caller.rs",
        "use crate::{Alpha, Beta, Gamma};\nuse std::path::Path;\n\npub fn make_alpha() -> Alpha {\n    Alpha::new()\n}\n\npub fn make_beta() -> Beta {\n    Beta::new()\n}\n\npub fn make_gamma() {\n    let _ = Gamma::new();\n}\n\npub fn make_path() {\n    let _ = Path::new(\"x\");\n    let _ = Vec::<u32>::new();\n}\n\npub fn make_library() -> u32 {\n    dunce::simplified(\"x\")\n}\n",
    ),
];

#[test]
fn a_path_call_through_a_type_that_does_not_own_the_target_is_excluded() {
    let repo = scanned_repo(RS_TYPES);
    let report = report(&repo, "Alpha.new", None);

    let alpha = line_of(&repo, "src/caller.rs", "Alpha::new()");
    assert_site(
        &report,
        "src/caller.rs",
        alpha,
        SiteLabel::Proven,
        "qualified path",
    );
    let beta = line_of(&repo, "src/lib.rs", "pub struct Beta") + 3;
    let line = line_of(&repo, "src/caller.rs", "Beta::new()");
    assert_site(
        &report,
        "src/caller.rs",
        line,
        SiteLabel::Excluded,
        &format!("resolves to `Beta.new` (src/lib.rs:{beta})"),
    );
    let line = line_of(&repo, "src/caller.rs", "Gamma::new()");
    assert_site(
        &report,
        "src/caller.rs",
        line,
        SiteLabel::Excluded,
        "receiver `Gamma` has type `Gamma`",
    );
}

#[test]
fn a_path_call_through_a_standard_library_import_is_excluded_with_its_module() {
    let repo = scanned_repo(RS_TYPES);
    let report = report(&repo, "Alpha.new", None);

    let line = line_of(&repo, "src/caller.rs", "Path::new");
    assert_site(
        &report,
        "src/caller.rs",
        line,
        SiteLabel::Excluded,
        "receiver `Path` is imported from `std::path`",
    );
}

#[test]
fn a_path_call_into_a_crate_the_index_lacks_is_excluded() {
    let repo = scanned_repo(RS_TYPES);
    let report = report(&repo, "simplified", None);

    let line = line_of(&repo, "src/caller.rs", "dunce::simplified");
    assert_site(
        &report,
        "src/caller.rs",
        line,
        SiteLabel::Excluded,
        "receiver `dunce` is imported from `dunce`",
    );
}

const RS_CRATES: &[(&str, &str)] = &[
    (
        "crates/a-core/src/lib.rs",
        "pub mod shapes;\n\npub struct Widget;\n\nimpl Widget {\n    pub fn build() -> Widget {\n        Widget\n    }\n}\n",
    ),
    (
        "crates/a-core/src/shapes.rs",
        "pub struct Square;\n\nimpl Square {\n    pub fn build() -> Square {\n        Square\n    }\n}\n",
    ),
    (
        "crates/a-cli/src/main.rs",
        "fn by_root() {\n    let _ = a_core::Widget::build();\n}\n\nfn by_module() {\n    let _ = a_core::shapes::Square::build();\n}\n\nfn by_other_crate() {\n    let _ = other_core::Widget::build();\n}\n\nfn main() {}\n",
    ),
];

#[test]
fn a_crate_qualified_path_to_a_method_is_proven_when_the_crate_and_owner_match() {
    let repo = scanned_repo(RS_CRATES);

    let widget = report(&repo, "Widget.build", None);
    let by_root = line_of(&repo, "crates/a-cli/src/main.rs", "a_core::Widget::build");
    assert_site(
        &widget,
        "crates/a-cli/src/main.rs",
        by_root,
        SiteLabel::Proven,
        "qualified path",
    );
    let other = line_of(
        &repo,
        "crates/a-cli/src/main.rs",
        "other_core::Widget::build",
    );
    assert_ne!(
        site_at(&widget, "crates/a-cli/src/main.rs", other).label,
        SiteLabel::Proven
    );

    let square = report(&repo, "Square.build", None);
    let by_module = line_of(
        &repo,
        "crates/a-cli/src/main.rs",
        "a_core::shapes::Square::build",
    );
    assert_site(
        &square,
        "crates/a-cli/src/main.rs",
        by_module,
        SiteLabel::Proven,
        "qualified path",
    );
    assert_eq!(
        site_at(&square, "crates/a-cli/src/main.rs", by_root).label,
        SiteLabel::Excluded
    );
}

const CTOR_JS: &[(&str, &str)] = &[(
    "src/thing.js",
    "class Thing {\n  constructor(x) {\n    this.x = x;\n  }\n}\nfunction make() {\n  return new Thing(1);\n}\n",
)];

#[test]
fn a_constructor_target_counts_a_same_file_new_expression() {
    let repo = scanned_repo(CTOR_JS);
    let report = report(&repo, "Thing.constructor", None);

    assert_eq!(report.text_scan.skipped.as_deref(), Some("Thing"));
    let line = line_of(&repo, "src/thing.js", "new Thing(1)");
    assert_site(
        &report,
        "src/thing.js",
        line,
        SiteLabel::Proven,
        "builds the class",
    );
}
