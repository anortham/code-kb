use code_kb_core::{
    LabeledSite, OverrideRow, ReferenceReport, SiteLabel, Workspace, find_julie_extract_binary,
    find_references_scoped, format_reference_report, get_symbol_by_id, open_read_only,
    reference_report, safe_tempdir, scan_workspace, select_reference_target,
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

fn first_definition_report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND kind NOT IN ('import', 'export')
             ORDER BY start_line LIMIT 1",
            [name, path],
            |row| row.get(0),
        )
        .unwrap();
    let target = get_symbol_by_id(&conn, &id).unwrap().unwrap();
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

fn assert_not_excluded(report: &ReferenceReport, path: &str, line: usize) {
    let site = site_at(report, path, line);
    assert_ne!(
        site.label,
        SiteLabel::Excluded,
        "{path}:{line} is excluded: {}\n{}",
        site.basis,
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
fn a_bare_call_that_several_files_could_define_is_unresolved() {
    let repo = scanned_repo(PY_MULTI);
    let report = report(&repo, "area", Some("shapes/a.py"));

    assert_site(
        &report,
        "app/main.py",
        2,
        SiteLabel::Unresolved,
        "3 definitions",
    );
}

#[test]
fn a_bare_call_that_only_overloads_define_twice_is_likely_by_name_only() {
    let repo = scanned_repo(&[
        (
            "lib/area.js",
            "function area(x) {\n  return 1;\n}\nfunction area(x, y) {\n  return 2;\n}\n",
        ),
        ("app/main.js", "function call() {\n  return area(1);\n}\n"),
    ]);
    let report = first_definition_report(&repo, "area", "lib/area.js");

    assert_site(&report, "app/main.js", 2, SiteLabel::Likely, "name only");
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
fn a_python_function_defined_inside_a_test_is_out_of_reach_of_other_files() {
    let repo = scanned_repo(&[
        (
            "tests/test_basic.py",
            "def test_session():\n    def read():\n        return 1\n\n    assert read() == 1\n",
        ),
        (
            "src/config.py",
            "def load(path):\n    with open(path) as f:\n        return f.read()\n",
        ),
    ]);
    let report = report(&repo, "read", Some("tests/test_basic.py"));

    assert_not_excluded(&report, "tests/test_basic.py", 5);
    assert_site(
        &report,
        "src/config.py",
        3,
        SiteLabel::Excluded,
        "`read` is local to `test_session`",
    );
    assert!(
        report
            .tests
            .iter()
            .all(|test| test.path == "tests/test_basic.py"),
        "{:?}",
        report.tests
    );
}

#[test]
fn a_python_class_defined_inside_a_test_is_no_other_definition_of_sites_outside_it() {
    let repo = scanned_repo(&[
        ("pkg/__init__.py", "from pkg.app import Flask\n"),
        (
            "pkg/app.py",
            "class Flask:\n    def __init__(self, name):\n        self.name = name\n",
        ),
        (
            "tests/test_appctx.py",
            "import pkg\n\n\ndef test_teardown(app: pkg.Flask) -> None:\n    assert app\n\n\ndef test_make():\n    app = pkg.Flask(__name__)\n    assert app\n",
        ),
        (
            "tests/test_config.py",
            "import pkg\n\n\ndef test_custom_config_class():\n    class Flask(pkg.Flask):\n        pass\n\n    app = Flask(__name__)\n    assert app\n",
        ),
    ]);
    let module = report(&repo, "Flask", Some("pkg/app.py"));
    let local = report(&repo, "Flask", Some("tests/test_config.py"));
    let annotation = line_of(&repo, "tests/test_appctx.py", "app: pkg.Flask");
    let inside = line_of(&repo, "tests/test_config.py", "app = Flask(");

    assert_site(
        &module,
        "tests/test_appctx.py",
        annotation,
        SiteLabel::Likely,
        "name only",
    );
    assert_site(
        &module,
        "tests/test_config.py",
        inside,
        SiteLabel::Excluded,
        "resolves to `Flask` (tests/test_config.py:5)",
    );
    assert_site(
        &local,
        "tests/test_appctx.py",
        annotation,
        SiteLabel::Excluded,
        "`Flask` is local to `test_custom_config_class`",
    );
    assert_site(
        &local,
        "tests/test_config.py",
        inside,
        SiteLabel::Proven,
        "enclosing scope",
    );
}

#[test]
fn the_constructor_of_a_python_class_defined_inside_a_test_is_out_of_reach_of_other_files() {
    let repo = scanned_repo(&[
        ("pkg/__init__.py", "from pkg.app import Flask\n"),
        (
            "pkg/app.py",
            "class Flask:\n    def __init__(self, name):\n        self.name = name\n",
        ),
        (
            "tests/test_appctx.py",
            "from pkg import *\n\n\ndef test_make():\n    app = Flask(__name__)\n    assert app\n",
        ),
        (
            "tests/test_config.py",
            "import pkg\n\n\ndef test_custom_config_class():\n    class Flask(pkg.Flask):\n        def __init__(self, name):\n            super().__init__(name)\n\n    app = Flask(__name__)\n    assert app\n",
        ),
    ]);
    let local_init = report(&repo, "__init__", Some("tests/test_config.py"));

    assert_site(
        &local_init,
        "tests/test_appctx.py",
        line_of(&repo, "tests/test_appctx.py", "app = Flask("),
        SiteLabel::Excluded,
        "`Flask` is local to `test_custom_config_class`",
    );
    assert_site(
        &local_init,
        "tests/test_config.py",
        line_of(&repo, "tests/test_config.py", "app = Flask("),
        SiteLabel::Proven,
        "builds the class",
    );
}

struct LocalTypeCase {
    files: &'static [(&'static str, &'static str)],
    module_path: &'static str,
    local_path: &'static str,
    outside: (&'static str, &'static str),
    inside: &'static str,
}

const LOCAL_TYPE_CASES: &[LocalTypeCase] = &[
    LocalTypeCase {
        files: &[
            (
                "src/main/java/p/Widget.java",
                "package p;\n\npublic class Widget {\n  public int size() { return 1; }\n}\n",
            ),
            (
                "src/main/java/p/Use.java",
                "package p;\n\nclass Use {\n  int outside() {\n    Widget a = new Widget();\n    return a.size();\n  }\n\n  int inside() {\n    class Widget {\n      int size() { return 2; }\n    }\n    Widget b = new Widget();\n    return b.size();\n  }\n}\n",
            ),
        ],
        module_path: "src/main/java/p/Widget.java",
        local_path: "src/main/java/p/Use.java",
        outside: ("src/main/java/p/Use.java", "Widget a = new Widget()"),
        inside: "Widget b = new Widget()",
    },
    LocalTypeCase {
        files: &[
            (
                "src/p/Widget.kt",
                "package p\n\nclass Widget {\n    fun size(): Int = 1\n}\n",
            ),
            (
                "src/p/Use.kt",
                "package p\n\nfun outside(): Int {\n    val a: Widget = Widget()\n    return a.size()\n}\n\nfun inside(): Int {\n    class Widget {\n        fun size(): Int = 2\n    }\n    val b: Widget = Widget()\n    return b.size()\n}\n",
            ),
        ],
        module_path: "src/p/Widget.kt",
        local_path: "src/p/Use.kt",
        outside: ("src/p/Use.kt", "val a: Widget"),
        inside: "val b: Widget",
    },
    LocalTypeCase {
        files: &[
            (
                "Sources/App/Widget.swift",
                "class Widget {\n    func size() -> Int { return 1 }\n}\n",
            ),
            (
                "Sources/App/Use.swift",
                "func outside() -> Int {\n    let a: Widget = Widget()\n    return a.size()\n}\n\nfunc inside() -> Int {\n    struct Widget {\n        func size() -> Int { return 2 }\n    }\n    let b: Widget = Widget()\n    return b.size()\n}\n",
            ),
        ],
        module_path: "Sources/App/Widget.swift",
        local_path: "Sources/App/Use.swift",
        outside: ("Sources/App/Use.swift", "let a: Widget"),
        inside: "let b: Widget",
    },
    LocalTypeCase {
        files: &[
            (
                "src/p/Widget.scala",
                "package p\n\nclass Widget {\n  def size(): Int = 1\n}\n",
            ),
            (
                "src/p/Use.scala",
                "package p\n\nobject Use {\n  def outside(): Int = {\n    val a: Widget = new Widget()\n    a.size()\n  }\n\n  def inside(): Int = {\n    class Widget {\n      def size(): Int = 2\n    }\n    val b: Widget = new Widget()\n    b.size()\n  }\n}\n",
            ),
        ],
        module_path: "src/p/Widget.scala",
        local_path: "src/p/Use.scala",
        outside: ("src/p/Use.scala", "val a: Widget"),
        inside: "val b: Widget",
    },
    LocalTypeCase {
        files: &[
            (
                "src/widget.rs",
                "pub struct Widget {\n    pub size: i32,\n}\n",
            ),
            (
                "src/lib.rs",
                "mod widget;\nuse crate::widget::Widget;\n\npub fn outside() -> i32 {\n    let a: Widget = Widget { size: 1 };\n    a.size\n}\n\npub fn inside() -> i32 {\n    struct Widget {\n        size: i32,\n    }\n    let b: Widget = Widget { size: 2 };\n    b.size\n}\n",
            ),
        ],
        module_path: "src/widget.rs",
        local_path: "src/lib.rs",
        outside: ("src/lib.rs", "let a: Widget"),
        inside: "let b: Widget",
    },
    LocalTypeCase {
        files: &[
            (
                "src/widget.h",
                "class Widget {\npublic:\n    int size() { return 1; }\n};\n",
            ),
            (
                "src/use.cpp",
                "#include \"widget.h\"\n\nint outside() {\n    Widget a;\n    return a.size();\n}\n\nint inside() {\n    struct Widget {\n        int size() { return 2; }\n    };\n    Widget b;\n    return b.size();\n}\n",
            ),
        ],
        module_path: "src/widget.h",
        local_path: "src/use.cpp",
        outside: ("src/use.cpp", "Widget a;"),
        inside: "Widget b;",
    },
    LocalTypeCase {
        files: &[
            ("src/widget.h", "struct Widget {\n    int size;\n};\n"),
            (
                "src/use.c",
                "#include \"widget.h\"\n\nint outside(void) {\n    struct Widget a = {1};\n    return a.size;\n}\n\nint inside(void) {\n    struct Widget {\n        int size;\n    };\n    struct Widget b = {2};\n    return b.size;\n}\n",
            ),
        ],
        module_path: "src/widget.h",
        local_path: "src/use.c",
        outside: ("src/use.c", "struct Widget a"),
        inside: "struct Widget b",
    },
    LocalTypeCase {
        files: &[
            (
                "p/widget.go",
                "package p\n\ntype Widget struct {\n\tSize int\n}\n",
            ),
            (
                "p/use.go",
                "package p\n\nfunc outside() int {\n\ta := Widget{Size: 1}\n\treturn a.Size\n}\n\nfunc inside() int {\n\ttype Widget struct {\n\t\tSize int\n\t}\n\tb := Widget{Size: 2}\n\treturn b.Size\n}\n",
            ),
        ],
        module_path: "p/widget.go",
        local_path: "p/use.go",
        outside: ("p/use.go", "a := Widget"),
        inside: "b := Widget",
    },
    LocalTypeCase {
        files: &[
            (
                "src/widget.zig",
                "pub const Widget = struct {\n    size: i32,\n};\n\nfn outside() i32 {\n    const a = Widget{ .size = 1 };\n    return a.size;\n}\n",
            ),
            (
                "src/local.zig",
                "fn inside() i32 {\n    const Widget = struct {\n        size: i32,\n    };\n    const b = Widget{ .size = 2 };\n    return b.size;\n}\n",
            ),
        ],
        module_path: "src/widget.zig",
        local_path: "src/local.zig",
        outside: ("src/widget.zig", "const a = Widget"),
        inside: "const b = Widget",
    },
];

#[test]
fn a_type_defined_inside_a_function_is_local_to_it() {
    for case in LOCAL_TYPE_CASES {
        let repo = scanned_repo(case.files);
        let module = report(&repo, "Widget", Some(case.module_path));
        let local = report(&repo, "Widget", Some(case.local_path));
        let outside = line_of(&repo, case.outside.0, case.outside.1);
        let inside = line_of(&repo, case.local_path, case.inside);

        assert_site(
            &local,
            case.outside.0,
            outside,
            SiteLabel::Excluded,
            "`Widget` is local to `inside`",
        );
        assert_not_excluded(&local, case.local_path, inside);
        let module_site = site_at(&module, case.outside.0, outside);
        assert!(
            !(module_site.label == SiteLabel::Unresolved
                || module_site.basis.contains("definitions")),
            "{}:{outside} counts the local type: {}\n{}",
            case.outside.0,
            module_site.basis,
            dump(&module)
        );
        assert_eq!(
            site_at(&module, case.local_path, inside).label,
            SiteLabel::Excluded,
            "{}",
            dump(&module)
        );
    }
}

#[test]
fn a_zig_receiver_imported_from_a_project_file_reaches_its_type() {
    let repo = scanned_repo(&[
        (
            "src/widget.zig",
            "pub const Widget = struct {\n    size: u32,\n};\n",
        ),
        (
            "src/main.zig",
            "const widget = @import(\"widget.zig\");\nconst std = @import(\"std\");\n\npub fn main() void {\n    const w = widget.Widget{ .size = 1 };\n    _ = w;\n    _ = std.Widget;\n}\n",
        ),
    ]);
    let report = report(&repo, "Widget", Some("src/widget.zig"));

    assert_not_excluded(&report, "src/main.zig", 5);
    assert_site(
        &report,
        "src/main.zig",
        7,
        SiteLabel::Excluded,
        "receiver `std` is imported from `std`",
    );
}

#[test]
fn a_java_field_read_never_reaches_a_method_of_that_name() {
    let repo = scanned_repo(&[
        (
            "src/main/java/p/Streams.java",
            "package p;\n\nclass Streams {\n  static class CurrentWrite {\n    int length() { return 0; }\n  }\n}\n",
        ),
        (
            "src/test/java/p/ArrayTest.java",
            "package p;\n\nclass ArrayTest {\n  void testArray() {\n    int[] target = new int[2];\n    assert target.length == 2;\n    Streams.CurrentWrite w = new Streams.CurrentWrite();\n    w.length();\n    java.util.function.IntSupplier s = w::length;\n  }\n}\n",
        ),
    ]);
    let report = report(&repo, "length", None);
    let path = "src/test/java/p/ArrayTest.java";

    assert_site(
        &report,
        path,
        6,
        SiteLabel::Excluded,
        "a member access reads a field, not the method `length`",
    );
    assert_not_excluded(&report, path, 8);
    assert_not_excluded(&report, path, 9);
}

#[test]
fn a_cpp_receiver_typed_by_a_template_parameter_is_not_excluded() {
    let repo = scanned_repo(&[
        (
            "include/alt_string.hpp",
            "class alt_string {\n  public:\n    void reserve(int n) {}\n};\n",
        ),
        (
            "include/concat.hpp",
            "template<typename OutStringType = std::string, typename... Args>\ninline OutStringType concat(Args && ... args)\n{\n    OutStringType str;\n    str.reserve(3);\n    return str;\n}\n",
        ),
    ]);
    let report = report(&repo, "reserve", Some("include/alt_string.hpp"));

    assert_not_excluded(&report, "include/concat.hpp", 5);
}

#[test]
fn a_call_keeps_its_own_line_when_julie_names_a_caller_that_ends_above_the_next_call() {
    let repo = scanned_repo(&[
        (
            "src/string_compat.erl",
            "-module(string_compat).\n-export([span/2]).\n\n-ifdef(OLD).\nspan(String, Chars) -> string:span(String, Chars).\n-else.\n\nspan(S, Cs) when is_list(Cs) -> span(S, Cs, 0).\n\nspan([C|S], Cs, I) ->\n    span(S, Cs, I+1);\nspan([], _Cs, I) -> I.\n-endif.\n",
        ),
        (
            "src/telemetry.erl",
            "-module(telemetry).\n-export([span/3]).\n\nspan(EventPrefix, StartMetadata, SpanFunction) ->\n    SpanFunction(EventPrefix, StartMetadata).\n",
        ),
    ]);
    let report = report(&repo, "span", Some("src/telemetry.erl"));

    let imported: Vec<usize> = report
        .sites
        .iter()
        .filter(|site| site.basis.contains("imported from `string`"))
        .map(|site| site.site.start_line.unwrap_or(0))
        .collect();
    assert_eq!(imported, [5], "{}", dump(&report));
}

#[test]
fn a_destructured_dynamic_import_binds_the_export_of_its_module() {
    let repo = scanned_repo(&[
        (
            "src/server.ts",
            "export async function hydrate(tool: string) {\n  return tool;\n}\n",
        ),
        (
            "src/other.ts",
            "export async function hydrate(tool: string) {\n  return tool + \"!\";\n}\n",
        ),
        (
            "tests/server.test.ts",
            "import { describe, it } from \"bun:test\";\n\ndescribe(\"hydrate\", () => {\n  it(\"fills a missing workspace\", async () => {\n    const { hydrate } = await import(\"../src/server\");\n    await hydrate(\"recall\");\n  });\n\n  it(\"uses the other module\", async () => {\n    const { hydrate } = await import(\"../src/other\");\n    await hydrate(\"brief\");\n  });\n});\n",
        ),
    ]);
    let report = report(&repo, "hydrate", Some("src/server.ts"));

    assert_site(
        &report,
        "tests/server.test.ts",
        6,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report,
        "tests/server.test.ts",
        11,
        SiteLabel::Excluded,
        "resolves to `hydrate` (src/other.ts:1)",
    );
    assert_eq!(report.counts.import_lines, 2, "{}", dump(&report));
}

#[test]
fn a_typescript_read_inside_the_block_of_its_binding_is_excluded_with_no_other_definition_near() {
    let repo = scanned_repo(&[
        (
            "src/locales/ru.ts",
            "export const error = () => \"ошибка\";\n",
        ),
        (
            "src/tests/error.test.ts",
            "import { error } from \"../locales/ru\";\n\nfunction getThrownError(fn: () => void) {\n  try {\n    fn();\n  } catch (error) {\n    return error;\n  }\n  return error();\n}\n\nfunction logAll(items: unknown[]) {\n  items.forEach((error) => {\n    console.log(error);\n  });\n}\n",
        ),
        (
            "src/tests/plain.test.ts",
            "function getRejectedError(fn: () => void) {\n  try {\n    fn();\n  } catch (error) {\n    return error;\n  }\n}\n",
        ),
    ]);
    let report = report(&repo, "error", Some("src/locales/ru.ts"));
    let binds = |line: usize| {
        format!(
            "line {line} binds the name as a callback parameter, loop variable, or catch parameter"
        )
    };

    assert_site(
        &report,
        "src/tests/error.test.ts",
        7,
        SiteLabel::Excluded,
        &binds(6),
    );
    assert_site(
        &report,
        "src/tests/error.test.ts",
        14,
        SiteLabel::Excluded,
        &binds(13),
    );
    assert_not_excluded(&report, "src/tests/error.test.ts", 9);
    assert_site(
        &report,
        "src/tests/plain.test.ts",
        5,
        SiteLabel::Excluded,
        &binds(4),
    );
}

#[test]
fn an_excluded_typescript_site_cites_the_binding_nearest_above_it() {
    let repo = scanned_repo(&[
        (
            "src/locales/ru.ts",
            "export const error = () => \"ошибка\";\n",
        ),
        (
            "src/tests/error.test.ts",
            "function getThrownError(fn: () => void) {\n  try {\n    fn();\n  } catch (error) {\n    return error;\n  }\n}\n\nfunction format(a: boolean, b: boolean) {\n  if (a) {\n    const error = 1;\n    use(error);\n  }\n  if (b) {\n    const error = 2;\n    use(error);\n  }\n}\n\nconst error = () => \"bound\";\n",
        ),
    ]);
    let report = report(&repo, "error", Some("src/locales/ru.ts"));
    let path = "src/tests/error.test.ts";

    assert_site(
        &report,
        path,
        5,
        SiteLabel::Excluded,
        "line 4 binds the name as a callback parameter, loop variable, or catch parameter",
    );
    let first = site_at(&report, path, 12);
    assert!(first.basis.ends_with(":11)"), "{}", dump(&report));
    let second = site_at(&report, path, 16);
    assert!(second.basis.ends_with(":15)"), "{}", dump(&report));
}

#[test]
fn a_test_that_only_a_bare_name_ties_to_the_target_is_marked_name_only() {
    let repo = scanned_repo(&[
        ("pkg/__init__.py", ""),
        ("pkg/shapes.py", "def render(shape):\n    return shape\n"),
        (
            "tests/test_shapes.py",
            "from pkg.shapes import render\n\n\ndef test_render():\n    assert render(1) == 1\n\n\ndef test_draw(canvas):\n    canvas.layer.render(2)\n",
        ),
    ]);
    let report = report(&repo, "render", Some("pkg/shapes.py"));

    let tests: Vec<(&str, bool)> = report
        .tests
        .iter()
        .map(|test| (test.name.as_str(), test.name_only))
        .collect();
    assert_eq!(
        tests,
        [("test_render", false), ("test_draw", true)],
        "{}",
        dump(&report)
    );
    assert!(
        format_reference_report(&report, 20)
            .contains("- `test_draw` [tests/test_shapes.py:8] (name only) run: pytest")
    );
}

#[test]
fn no_site_reaches_a_mocha_describe_label() {
    let repo = scanned_repo(&[(
        "test/res.attachment.js",
        "var express = require('../');\n\ndescribe('res', function(){\n  it('should set Content-Disposition', function(done){\n    var app = express();\n    app.use(function(req, res){\n      res.attachment();\n      res.end();\n    });\n    done();\n  });\n});\n",
    )]);
    let report = report(&repo, "res", Some("test/res.attachment.js"));

    assert!(!report.sites.is_empty(), "{}", dump(&report));
    assert!(
        report
            .sites
            .iter()
            .all(|site| site.label == SiteLabel::Excluded
                && site.basis == "`res` is a test label, which code never names"),
        "{}",
        dump(&report)
    );
    assert!(report.tests.is_empty());
}

#[test]
fn the_first_body_line_of_a_ruby_method_is_not_a_definition_line() {
    let repo = scanned_repo(&[(
        "lib/base.rb",
        "class Base\n  def prototype\n    @prototype ||= new\n  end\nend\n",
    )]);
    let report = report(&repo, "prototype", None);

    assert_eq!(report.counts.definition_lines, 1, "{}", dump(&report));
    assert_eq!(report.counts.text_only_lines, 1);
    assert_counts_add_up(&report);
}

#[test]
fn a_one_line_definition_after_its_attribute_counts_its_name_line_as_a_definition_line() {
    let repo = scanned_repo(&[
        (
            "src/Shapes.fs",
            "module Shapes\n\n[<Struct>]\ntype Point = { X: int; Y: int }\n",
        ),
        (
            "app/views.py",
            "def route(f):\n    return f\n\n\n@route\ndef index(): return 1\n",
        ),
    ]);

    for (name, path) in [("Point", "src/Shapes.fs"), ("index", "app/views.py")] {
        let report = first_definition_report(&repo, name, path);
        assert_eq!(report.counts.definition_lines, 1, "{}", dump(&report));
        assert_eq!(report.counts.text_only_lines, 0, "{}", dump(&report));
    }
}

#[test]
fn an_annotated_kotlin_function_counts_its_name_line_as_a_definition_line() {
    let repo = scanned_repo(&[(
        "src/main/kotlin/com/example/Util.kt",
        "package com.example\n\n@PublishedApi\ninternal fun <T> Class<T>.boxIfPrimitive(): Class<T> {\n  return this\n}\n",
    )]);
    let report = report(&repo, "boxIfPrimitive", None);

    assert_eq!(report.counts.definition_lines, 1, "{}", dump(&report));
    assert_eq!(report.counts.text_only_lines, 0);
}

#[test]
fn a_kotlin_import_named_by_its_full_path_counts_as_an_import_line() {
    let repo = scanned_repo(&[
        (
            "src/main/kotlin/com/example/internal/Util.kt",
            "package com.example.internal\n\nfun boxIfPrimitive(x: Int): Int = x\n",
        ),
        (
            "src/main/kotlin/com/example/Use.kt",
            "package com.example\n\nimport com.example.internal.boxIfPrimitive\n\nfun use(): Int = boxIfPrimitive(1)\n",
        ),
    ]);
    let report = report(&repo, "boxIfPrimitive", None);

    assert_eq!(report.counts.import_lines, 1, "{}", dump(&report));
    assert_eq!(report.counts.text_only_lines, 0);
    assert_counts_add_up(&report);
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
        vec!["calls through getattr() or names built at runtime"]
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

#[test]
fn a_javascript_read_inside_a_callback_body_is_excluded_and_one_outside_is_likely() {
    let repo = scanned_repo(&[(
        "app/index.js",
        "function count(req, res, next) {\n  next();\n}\n\napp.get('/', function (err, count) {\n  res.send({ total: count });\n});\n\napp.use(count);\n",
    )]);
    let report = report(&repo, "count", None);

    let shadowed = line_of(&repo, "app/index.js", "total: count");
    assert_site(
        &report,
        "app/index.js",
        shadowed,
        SiteLabel::Excluded,
        "line 5 binds the name as a callback parameter, loop variable, or catch parameter",
    );
    let passed = line_of(&repo, "app/index.js", "app.use(count)");
    assert_site(
        &report,
        "app/index.js",
        passed,
        SiteLabel::Likely,
        "same file; the index does not record callback parameters",
    );
}

#[test]
fn a_bare_call_never_reaches_a_javascript_property_method() {
    let repo = scanned_repo(&[(
        "lib/request.js",
        "var accepts = require('accepts');\nvar req = module.exports = {};\n\nreq.accepts = function () {\n  var accept = accepts(this);\n  return accept.types();\n};\n",
    )]);
    let report = report(&repo, "accepts", Some("lib/request.js"));

    let line = line_of(&repo, "lib/request.js", "accepts(this)");
    assert_site(
        &report,
        "lib/request.js",
        line,
        SiteLabel::Excluded,
        "resolves to the import of `accepts` from `accepts`",
    );
}

const TS_OBJECT_LITERAL_METHOD: &[(&str, &str)] = &[
    ("package.json", "{\"name\": \"demo\"}\n"),
    (
        "src/schemas.ts",
        "declare function install(inst: object, methods: object): void;\ndeclare function make(name: string, init: (inst: object) => void): object;\n\nexport const ZodType = make(\"ZodType\", (inst) => {\n  install(inst, {\n    array() {\n      return array(this);\n    },\n  });\n});\n\nexport function array(element: unknown) {\n  return [element];\n}\n",
    ),
    ("src/index.ts", "export * from \"./schemas\";\n"),
    (
        "src/use.ts",
        "import * as z from \"./index\";\n\nexport function usesArray() {\n  return z.array(1);\n}\n",
    ),
];

#[test]
fn a_namespace_import_reaches_the_module_function_and_not_an_object_literal_method_with_its_name() {
    let repo = scanned_repo(TS_OBJECT_LITERAL_METHOD);
    let method = report_at(&repo, "array", "src/schemas.ts", 6);
    let function = report_at(&repo, "array", "src/schemas.ts", 12);

    let namespace_call = line_of(&repo, "src/use.ts", "z.array(1)");
    let bare_call = line_of(&repo, "src/schemas.ts", "array(this)");
    assert_eq!(
        site_at(&method, "src/use.ts", namespace_call).label,
        SiteLabel::Excluded
    );
    assert_site(
        &function,
        "src/use.ts",
        namespace_call,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &function,
        "src/schemas.ts",
        bare_call,
        SiteLabel::Proven,
        "same file",
    );
}

#[test]
fn a_bare_java_call_through_a_static_import_of_the_class_file_is_proven() {
    let repo = scanned_repo(&[
        (
            "src/main/java/p/util/Checks.java",
            "package p.util;\n\npublic class Checks {\n    public static void check(boolean b) {\n    }\n}\n",
        ),
        (
            "src/main/java/q/util/Checks.java",
            "package q.util;\n\npublic class Checks {\n    public static void check(boolean b) {\n    }\n}\n",
        ),
        (
            "src/main/java/p/Use.java",
            "package p;\n\nimport static p.util.Checks.check;\n\npublic class Use {\n    void viaImport() {\n        check(false);\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "check", Some("src/main/java/p/util/Checks.java"));

    assert_site(
        &report,
        "src/main/java/p/Use.java",
        7,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_java_static_import_with_a_rival_owner_keeps_bare_calls_unresolved() {
    let repo = scanned_repo(&[
        (
            "p/Util.java",
            "package p;\n\npublic class Util {\n    public static void check(boolean value) {\n    }\n}\n",
        ),
        (
            "q/Other.java",
            "package q;\n\npublic class Other {\n    public static void check(String value) {\n    }\n}\n",
        ),
        (
            "r/ExplicitUtilFirst.java",
            "package r;\n\nimport static p.Util.check;\nimport static q.Other.check;\n\nclass ExplicitUtilFirst {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
        (
            "r/ExplicitOtherFirst.java",
            "package r;\n\nimport static q.Other.check;\nimport static p.Util.check;\n\nclass ExplicitOtherFirst {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
        (
            "r/ExplicitThenOtherWildcard.java",
            "package r;\n\nimport static p.Util.check;\nimport static q.Other.*;\n\nclass ExplicitThenOtherWildcard {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
        (
            "r/OtherWildcardThenExplicit.java",
            "package r;\n\nimport static q.Other.*;\nimport static p.Util.check;\n\nclass OtherWildcardThenExplicit {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
        (
            "r/TargetWildcardThenOtherExplicit.java",
            "package r;\n\nimport static p.Util.*;\nimport static q.Other.check;\n\nclass TargetWildcardThenOtherExplicit {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
        (
            "r/OtherExplicitThenTargetWildcard.java",
            "package r;\n\nimport static q.Other.check;\nimport static p.Util.*;\n\nclass OtherExplicitThenTargetWildcard {\n    void run() {\n        check(true);\n        check(\"hello\");\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "check", Some("p/Util.java"));

    for path in [
        "r/ExplicitUtilFirst.java",
        "r/ExplicitOtherFirst.java",
        "r/ExplicitThenOtherWildcard.java",
        "r/OtherWildcardThenExplicit.java",
        "r/TargetWildcardThenOtherExplicit.java",
        "r/OtherExplicitThenTargetWildcard.java",
    ] {
        for line in [8, 9] {
            assert_eq!(
                site_at(&report, path, line).label,
                SiteLabel::Unresolved,
                "{path}:{line}\n{}",
                dump(&report)
            );
        }
    }
}

#[test]
fn a_bare_java_call_reaches_only_its_classes_and_its_static_imports() {
    let repo = scanned_repo(&[
        (
            "src/main/java/p/common/MoreAsserts.java",
            "package p.common;\n\npublic final class MoreAsserts {\n    public static void assertEquals(Object a, Object b) {\n    }\n}\n",
        ),
        (
            "src/main/java/p/Inherited.java",
            "package p;\n\nimport junit.framework.TestCase;\n\npublic class Inherited extends TestCase {\n    public void run() {\n        assertEquals(1, 1);\n    }\n}\n",
        ),
        (
            "src/main/java/p/Named.java",
            "package p;\n\nimport static p.common.MoreAsserts.assertEquals;\n\npublic class Named {\n    public void run() {\n        assertEquals(1, 1);\n    }\n}\n",
        ),
        (
            "src/main/java/p/Wildcard.java",
            "package p;\n\nimport static p.common.MoreAsserts.*;\n\npublic class Wildcard {\n    public void run() {\n        assertEquals(1, 1);\n    }\n}\n",
        ),
    ]);
    let report = report(
        &repo,
        "assertEquals",
        Some("src/main/java/p/common/MoreAsserts.java"),
    );

    assert_site(
        &report,
        "src/main/java/p/Inherited.java",
        7,
        SiteLabel::Excluded,
        "a bare call reaches only the classes of `Inherited` and its static imports",
    );
    assert_site(
        &report,
        "src/main/java/p/Named.java",
        7,
        SiteLabel::Proven,
        "import",
    );
    assert_not_excluded(&report, "src/main/java/p/Wildcard.java", 7);
}

#[test]
fn a_bare_java_call_in_an_anonymous_class_reaches_its_supertype() {
    let repo = scanned_repo(&[
        (
            "src/main/java/p/Adapter.java",
            "package p;\n\npublic abstract class Adapter {\n    public final void nullSafe() {\n    }\n\n    public abstract void write();\n}\n",
        ),
        (
            "src/main/java/p/Adapters.java",
            "package p;\n\npublic final class Adapters {\n    static final Adapter FIELD = new Adapter() {\n        @Override\n        public void write() {\n            nullSafe();\n        }\n    };\n\n    void build() {\n        Adapter local = new Adapter() {\n            @Override\n            public void write() {\n                nullSafe();\n            }\n        };\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "nullSafe", Some("src/main/java/p/Adapter.java"));

    assert_not_excluded(&report, "src/main/java/p/Adapters.java", 7);
    assert_not_excluded(&report, "src/main/java/p/Adapters.java", 15);
}

#[test]
fn a_java_annotation_element_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        (
            "src/main/java/p/Name.java",
            "package p;\n\npublic @interface Name {\n    String value();\n\n    String[] alternate() default {};\n}\n",
        ),
        (
            "src/main/java/p/Use.java",
            "package p;\n\npublic class Use {\n    @Name(value = \"a\",\n        alternate = {\"b\"})\n    String field;\n}\n",
        ),
    ]);
    let report = report(&repo, "alternate", Some("src/main/java/p/Name.java"));

    assert_not_excluded(&report, "src/main/java/p/Use.java", 5);
}

#[test]
fn a_decorator_line_of_a_property_setter_is_a_definition_line() {
    let repo = scanned_repo(&[(
        "pkg/box.py",
        "class Box:\n    @property\n    def size(self):\n        return 1\n\n    @size.setter\n    def size(self, value):\n        pass\n",
    )]);
    let report = first_definition_report(&repo, "size", "pkg/box.py");

    let decorator = line_of(&repo, "pkg/box.py", "@size.setter");
    assert!(
        report
            .sites
            .iter()
            .all(|site| site.site.start_line != Some(decorator)),
        "{}",
        dump(&report)
    );
    assert_counts_add_up(&report);
}

const PY_STDLIB_JSON: &[(&str, &str)] = &[
    ("pkg/__init__.py", ""),
    ("pkg/json/__init__.py", "def dumps(obj):\n    return obj\n"),
    (
        "pkg/json/provider.py",
        "import json\n\n\nclass Provider:\n    def dumps(self, obj):\n        return json.dumps(obj)\n",
    ),
];

#[test]
fn a_module_receiver_never_proves_a_class_member() {
    let repo = scanned_repo(PY_STDLIB_JSON);
    let report = report(&repo, "Provider.dumps", None);

    let line = line_of(&repo, "pkg/json/provider.py", "json.dumps(obj)");
    assert_ne!(
        site_at(&report, "pkg/json/provider.py", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_python_module_receiver_reaches_no_class_member_with_the_name() {
    let mut files = PY_STDLIB_JSON.to_vec();
    files.push((
        "tests/test_json.py",
        "import pkg\nfrom pkg import json\n\n\ndef test_dumps():\n    pkg.json.dumps(1)\n    json.dumps(2)\n",
    ));
    let repo = scanned_repo(&files);
    let report = report(&repo, "dumps", Some("pkg/json/__init__.py"));

    for line in [6, 7] {
        assert!(
            matches!(
                site_at(&report, "tests/test_json.py", line).label,
                SiteLabel::Proven | SiteLabel::Likely
            ),
            "{}",
            dump(&report)
        );
    }
}

#[test]
fn a_python_absolute_import_proves_only_a_file_of_its_top_package() {
    let repo = scanned_repo(PY_STDLIB_JSON);
    let report = report(&repo, "dumps", Some("pkg/json/__init__.py"));

    let line = line_of(&repo, "pkg/json/provider.py", "json.dumps(obj)");
    assert_ne!(
        site_at(&report, "pkg/json/provider.py", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_package_import_that_reaches_two_definitions_is_unresolved() {
    let repo = scanned_repo(&[
        (
            "packages/lib/package.json",
            "{\n  \"name\": \"lib\",\n  \"main\": \"./src/index.ts\"\n}\n",
        ),
        (
            "packages/lib/src/schemas.ts",
            "export function boolean() {\n  return 1;\n}\n",
        ),
        (
            "packages/lib/src/coerce.ts",
            "export function boolean() {\n  return 2;\n}\n",
        ),
        (
            "packages/lib/src/index.ts",
            "export * from './schemas';\nexport * from './coerce';\n",
        ),
        (
            "packages/app/use.ts",
            "import * as z from 'lib';\n\nexport const s = z.boolean();\n",
        ),
    ]);
    let report = report(&repo, "boolean", Some("packages/lib/src/coerce.ts"));

    let line = line_of(&repo, "packages/app/use.ts", "z.boolean()");
    assert_site(
        &report,
        "packages/app/use.ts",
        line,
        SiteLabel::Unresolved,
        "the import reaches 2 definitions",
    );
}

#[test]
fn a_call_julie_links_to_its_own_caller_is_not_proven() {
    let repo = scanned_repo(&[
        (
            "src/Reader.cs",
            "namespace N\n{\n    public class Reader\n    {\n        private void AddReference(string id)\n        {\n            Resolver().AddReference(this, id);\n        }\n\n        private Other Resolver()\n        {\n            return null;\n        }\n    }\n}\n",
        ),
        (
            "src/Other.cs",
            "namespace N\n{\n    public class Other\n    {\n        public void AddReference(object o, string id)\n        {\n        }\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "Reader.AddReference", None);

    let line = line_of(&repo, "src/Reader.cs", "Resolver().AddReference");
    assert_ne!(
        site_at(&report, "src/Reader.cs", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_subclass_write_to_an_inherited_property_is_a_site_not_a_definition() {
    let repo = scanned_repo(&[
        ("pkg/__init__.py", ""),
        (
            "pkg/base.py",
            "class Base:\n    @property\n    def debug(self):\n        return self._debug\n\n    @debug.setter\n    def debug(self, value):\n        self._debug = value\n",
        ),
        (
            "pkg/app.py",
            "from pkg.base import Base\n\n\nclass App(Base):\n    def __init__(self):\n        self.debug = False\n\n    def run(self):\n        return self.debug\n",
        ),
    ]);
    let report = first_definition_report(&repo, "debug", "pkg/base.py");

    let read = line_of(&repo, "pkg/app.py", "return self.debug");
    assert_site(
        &report,
        "pkg/app.py",
        read,
        SiteLabel::Proven,
        "class chain",
    );
    let write = line_of(&repo, "pkg/app.py", "self.debug = False");
    assert_eq!(
        site_at(&report, "pkg/app.py", write).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_csharp_extension_method_call_through_the_extended_type_is_not_excluded() {
    let repo = scanned_repo(&[
        (
            "src/AsyncUtils.cs",
            "namespace N\n{\n    public static class AsyncUtils\n    {\n        public static int FromCanceled(this CancellationToken token)\n        {\n            return 0;\n        }\n    }\n}\n",
        ),
        (
            "src/Writer.cs",
            "namespace N\n{\n    public class Writer\n    {\n        public int Flush(CancellationToken cancellationToken)\n        {\n            return cancellationToken.FromCanceled();\n        }\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "FromCanceled", None);

    let line = line_of(&repo, "src/Writer.cs", "cancellationToken.FromCanceled()");
    assert_site(
        &report,
        "src/Writer.cs",
        line,
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_test_label_is_not_a_definition_of_its_name() {
    let repo = scanned_repo(&[
        (
            "packages/lib/package.json",
            "{\n  \"name\": \"lib\",\n  \"main\": \"./src/index.ts\"\n}\n",
        ),
        (
            "packages/lib/src/schemas.ts",
            "export function strictObject() {\n  return 1;\n}\n",
        ),
        ("packages/lib/src/index.ts", "export * from './schemas';\n"),
        (
            "packages/lib/src/tests/object.test.ts",
            "import { test } from 'vitest';\nimport * as z from 'lib';\n\ntest('strictObject', () => {\n  z.strictObject();\n});\n",
        ),
        (
            "packages/lib/src/tests/other.test.ts",
            "import { test } from 'vitest';\nimport * as z from 'lib';\n\ntest('other', () => {\n  z.strictObject();\n});\n",
        ),
    ]);
    let report = report(&repo, "strictObject", Some("packages/lib/src/schemas.ts"));

    for path in [
        "packages/lib/src/tests/object.test.ts",
        "packages/lib/src/tests/other.test.ts",
    ] {
        let line = line_of(&repo, path, "z.strictObject()");
        assert_site(&report, path, line, SiteLabel::Proven, "import");
    }
}

#[test]
fn an_object_member_does_not_claim_a_call_whose_qualifier_imports_another_package() {
    let repo = scanned_repo(&[
        (
            "packages/v3/types.ts",
            "export const coerce = {\n  boolean: () => 1,\n};\n",
        ),
        (
            "packages/v4/package.json",
            "{\n  \"name\": \"v4\",\n  \"main\": \"./index.ts\"\n}\n",
        ),
        (
            "packages/v4/coerce.ts",
            "export function boolean() {\n  return 2;\n}\n",
        ),
        (
            "packages/v4/index.ts",
            "export * as coerce from './coerce';\n",
        ),
        (
            "packages/app/use.ts",
            "import * as z from 'v4';\n\nexport const b = z.coerce.boolean();\n",
        ),
    ]);
    let report = report(&repo, "boolean", Some("packages/v4/coerce.ts"));

    let line = line_of(&repo, "packages/app/use.ts", "z.coerce.boolean()");
    assert_site(
        &report,
        "packages/app/use.ts",
        line,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_java_call_on_a_call_result_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        (
            "src/p/Element.java",
            "package p;\n\npublic abstract class Element {\n    public int getAsInt() {\n        return 0;\n    }\n}\n",
        ),
        (
            "src/p/Array.java",
            "package p;\n\npublic class Array extends Element {\n    private Element single() {\n        return null;\n    }\n\n    @Override\n    public int getAsInt() {\n        return single().getAsInt();\n    }\n}\n",
        ),
    ]);
    let report = first_definition_report(&repo, "getAsInt", "src/p/Element.java");

    let line = line_of(&repo, "src/p/Array.java", "single().getAsInt()");
    assert_site(
        &report,
        "src/p/Array.java",
        line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_python_self_call_that_julie_links_in_the_same_file_stays_proven() {
    let repo = scanned_repo(&[(
        "pkg/app.py",
        "class App:\n    def ensure_sync(self, func):\n        return func\n\n    def run(self, func):\n        return self.ensure_sync(func)()\n",
    )]);
    let report = report(&repo, "ensure_sync", None);

    let line = line_of(&repo, "pkg/app.py", "self.ensure_sync(func)");
    assert_eq!(
        site_at(&report, "pkg/app.py", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

const GO_FIELDS: &[(&str, &str)] = &[
    (
        "a/name.go",
        "package a\n\nfunc Name() string {\n\treturn \"a\"\n}\n",
    ),
    (
        "b/cmd.go",
        "package b\n\ntype Command struct {\n\tName string\n}\n",
    ),
    (
        "c/use.go",
        "package c\n\nfunc call(x any) string {\n\treturn x.Name()\n}\n\nfunc read(x any) string {\n\treturn x.Name\n}\n",
    ),
];

#[test]
fn a_go_call_does_not_count_a_struct_field_as_a_rival() {
    let repo = scanned_repo(GO_FIELDS);
    let report = report(&repo, "Name", Some("a/name.go"));

    let line = line_of(&repo, "c/use.go", "x.Name()");
    assert_site(&report, "c/use.go", line, SiteLabel::Likely, "name only");
}

#[test]
fn a_go_member_read_keeps_a_struct_field_as_a_rival() {
    let repo = scanned_repo(GO_FIELDS);
    let report = report(&repo, "Name", Some("a/name.go"));

    let line = line_of(&repo, "c/use.go", "func read") + 1;
    assert_site(
        &report,
        "c/use.go",
        line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_go_field_of_a_func_type_stays_a_rival_of_a_call() {
    let repo = scanned_repo(&[
        (
            "a/fns.go",
            "package a\n\nfunc Run(n int) {}\n\nfunc Handle(n int) {}\n\nfunc Size() int {\n\treturn 1\n}\n",
        ),
        (
            "b/cmd.go",
            "package b\n\ntype HandlerFunc func(n int) error\n\ntype Command struct {\n\tRun    func(n int)\n\tHandle HandlerFunc\n\tSize   int\n}\n",
        ),
        (
            "c/use.go",
            "package c\n\nfunc call(x any) {\n\tx.Run(1)\n\tx.Handle(2)\n\tx.Size()\n}\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "c/use.go", needle);

    let run = report(&repo, "Run", Some("a/fns.go"));
    assert_site(
        &run,
        "c/use.go",
        line("x.Run"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let handle = report(&repo, "Handle", Some("a/fns.go"));
    assert_site(
        &handle,
        "c/use.go",
        line("x.Handle"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let size = report(&repo, "Size", Some("a/fns.go"));
    assert_site(
        &size,
        "c/use.go",
        line("x.Size"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_java_call_does_not_count_a_property_as_a_rival() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    int size() {\n        return 1;\n    }\n\n    int size(int x) {\n        return x;\n    }\n}\n",
        ),
        (
            "q/B.java",
            "package q;\n\npublic class B {\n    int size;\n\n    int go() {\n        return make().size();\n    }\n}\n",
        ),
    ]);
    let report = first_definition_report(&repo, "size", "p/A.java");

    let line = line_of(&repo, "q/B.java", "make().size()");
    assert_site(&report, "q/B.java", line, SiteLabel::Likely, "name only");
}

#[test]
fn a_rust_call_does_not_count_a_field_as_a_rival() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub struct S {\n    pub name: String,\n}\n\nimpl S {\n    pub fn name(&self) -> &str {\n        &self.name\n    }\n}\n\nfn go() {\n    make().name();\n}\n",
    )]);
    let report = report(&repo, "name", Some("src/lib.rs"));

    let line = line_of(&repo, "src/lib.rs", "make().name()");
    assert_site(&report, "src/lib.rs", line, SiteLabel::Likely, "name only");
}

const JS_VIEW: (&str, &str) = (
    "lib/view.js",
    "class View {\n  render() {\n    return 1;\n  }\n}\n",
);
const JS_MEMBER_CALL: (&str, &str) = ("lib/use.js", "function go(v) {\n  return v.render();\n}\n");

#[test]
fn a_javascript_member_call_ignores_a_free_function_of_the_same_name() {
    let repo = scanned_repo(&[
        JS_VIEW,
        JS_MEMBER_CALL,
        (
            "lib/util.js",
            "function render(x) {\n  return x;\n}\nmodule.exports = {};\n",
        ),
    ]);
    let report = report(&repo, "render", Some("lib/view.js"));

    assert_site(&report, "lib/use.js", 2, SiteLabel::Likely, "name only");
}

#[test]
fn a_javascript_member_call_keeps_an_exported_function_as_a_rival() {
    let repo = scanned_repo(&[
        JS_VIEW,
        JS_MEMBER_CALL,
        (
            "lib/util.js",
            "export function render(x) {\n  return x;\n}\n",
        ),
    ]);
    let report = report(&repo, "render", Some("lib/view.js"));

    assert_site(
        &report,
        "lib/use.js",
        2,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_javascript_member_call_keeps_a_function_assigned_on_an_object_as_a_rival() {
    let repo = scanned_repo(&[
        JS_VIEW,
        JS_MEMBER_CALL,
        (
            "lib/app.js",
            "var app = {};\napp.render = function render(name) {\n  return name;\n};\n",
        ),
    ]);
    let report = report(&repo, "render", Some("lib/view.js"));

    assert_site(
        &report,
        "lib/use.js",
        2,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_javascript_member_call_keeps_a_function_its_own_file_uses_as_a_member() {
    let repo = scanned_repo(&[
        JS_VIEW,
        JS_MEMBER_CALL,
        (
            "lib/util.js",
            "var api = {};\nfunction render(x) {\n  return x;\n}\napi.render = render;\nmodule.exports = {};\n",
        ),
    ]);
    let report = report(&repo, "render", Some("lib/view.js"));

    assert_site(
        &report,
        "lib/use.js",
        2,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_call_on_a_line_that_continues_a_chain_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        (
            "lib/schema.js",
            "class Schema {\n  readonly() {\n    return this;\n  }\n}\n",
        ),
        (
            "lib/use.js",
            "function readonly(x) {\n  return x;\n}\n\nfunction build(schema) {\n  return make(schema)\n    .readonly();\n}\nmodule.exports = { build };\n",
        ),
    ]);
    let report = report(&repo, "readonly", Some("lib/schema.js"));

    let line = line_of(&repo, "lib/use.js", ".readonly()");
    assert_site(&report, "lib/use.js", line, SiteLabel::Likely, "name only");
}

const JS_RESPONSE: (&str, &str) = (
    "lib/response.js",
    "var res = Object.create(proto);\nmodule.exports = res;\n\nres.redirect = function redirect(url) {\n  return url;\n};\n",
);
const JS_REDIRECTS: (&str, &str) = (
    "lib/use.js",
    "function go(res, r) {\n  res.redirect('/a');\n  r.redirect('/b');\n}\n",
);

#[test]
fn a_javascript_receiver_named_for_the_object_a_function_is_assigned_on_is_likely() {
    let repo = scanned_repo(&[
        JS_RESPONSE,
        JS_REDIRECTS,
        (
            "lib/mock.js",
            "var mock = {};\nmock.redirect = function redirect(url) {\n  return url;\n};\n",
        ),
    ]);
    let report = report(&repo, "redirect", Some("lib/response.js"));

    assert_site(
        &report,
        "lib/use.js",
        2,
        SiteLabel::Likely,
        "receiver `res` names the object `redirect` is assigned on",
    );
    assert_site(
        &report,
        "lib/use.js",
        3,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_receiver_named_for_the_object_stays_unresolved_when_a_rival_is_assigned_on_the_same_name() {
    let repo = scanned_repo(&[
        JS_RESPONSE,
        JS_REDIRECTS,
        (
            "lib/other.js",
            "var res = {};\nres.redirect = function redirect(url) {\n  return url;\n};\n",
        ),
    ]);
    let report = report(&repo, "redirect", Some("lib/response.js"));

    assert_site(
        &report,
        "lib/use.js",
        2,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

const JS_LOCAL_MOCK: &[(&str, &str)] = &[
    (
        "lib/response.js",
        "var res = Object.create(proto);\nmodule.exports = res;\n\nres.send = function send(body) {\n  return body;\n};\n",
    ),
    (
        "test/router.js",
        "it('dispatches', function () {\n  var res = {\n    send: function (val) {\n      return val;\n    }\n  };\n  res.send(1);\n});\n",
    ),
    (
        "test/other.js",
        "it('sends', function () {\n  return request(app, function (req, res) {\n    res.send(2);\n  });\n});\n",
    ),
];

#[test]
fn a_method_of_a_local_object_never_excludes_a_receiver_outside_its_scope() {
    let repo = scanned_repo(JS_LOCAL_MOCK);
    let report = report(&repo, "send", Some("lib/response.js"));

    assert_site(
        &report,
        "test/other.js",
        3,
        SiteLabel::Likely,
        "receiver `res` names the object `send` is assigned on",
    );
    assert_site(
        &report,
        "test/router.js",
        7,
        SiteLabel::Excluded,
        "resolves to `send` (test/router.js:3)",
    );
}

#[test]
fn a_method_of_a_local_object_ties_only_the_receivers_inside_its_scope() {
    let repo = scanned_repo(JS_LOCAL_MOCK);
    let report = report(&repo, "send", Some("test/router.js"));

    assert_site(
        &report,
        "test/router.js",
        7,
        SiteLabel::Proven,
        "qualified path",
    );
    assert_site(
        &report,
        "test/other.js",
        3,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

const JS_NESTED_LOCAL_MOCK: &[(&str, &str)] = &[
    (
        "lib/response.js",
        "var res = Object.create(proto);\nmodule.exports = res;\n\nres.send = function send(body) {\n  return body;\n};\n",
    ),
    (
        "test/router.js",
        "it('dispatches', function () {\n  var ctx = {\n    res: {\n      send: function (val) {\n        return val;\n      }\n    }\n  };\n  ctx.res.send(1);\n});\n",
    ),
    (
        "test/other.js",
        "it('sends', function () {\n  return request(app, function (req, res) {\n    res.send(2);\n  });\n});\n",
    ),
];

#[test]
fn a_method_of_a_nested_local_object_never_excludes_a_receiver_outside_its_scope() {
    let repo = scanned_repo(JS_NESTED_LOCAL_MOCK);
    let report = report(&repo, "send", Some("lib/response.js"));

    assert_site(
        &report,
        "test/other.js",
        3,
        SiteLabel::Likely,
        "receiver `res` names the object `send` is assigned on",
    );
    assert_site(
        &report,
        "test/router.js",
        9,
        SiteLabel::Excluded,
        "resolves to `send` (test/router.js:4)",
    );
}

#[test]
fn a_method_of_a_nested_local_object_ties_only_the_receivers_inside_its_scope() {
    let repo = scanned_repo(JS_NESTED_LOCAL_MOCK);
    let report = report(&repo, "send", Some("test/router.js"));

    assert_site(
        &report,
        "test/router.js",
        9,
        SiteLabel::Proven,
        "qualified path",
    );
    assert_site(
        &report,
        "test/other.js",
        3,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

const JS_RETURNED_LOCAL_OBJECT: &[(&str, &str)] = &[
    (
        "lib/bus.js",
        "var bus = {};\nmodule.exports = bus;\n\nbus.dispatch = function dispatch(event) {\n  return event;\n};\n",
    ),
    (
        "lib/store.js",
        "function createStore() {\n  const store = {\n    dispatch(action) {\n      return action;\n    },\n  };\n  return store;\n}\nmodule.exports = createStore;\n",
    ),
    (
        "src/app.js",
        "const createStore = require('../lib/store');\nfunction main() {\n  const store = createStore();\n  store.dispatch(1);\n}\nmodule.exports = main;\n",
    ),
];

#[test]
fn a_returned_local_object_stays_a_rival_outside_its_scope() {
    let repo = scanned_repo(JS_RETURNED_LOCAL_OBJECT);
    let bus = report(&repo, "dispatch", Some("lib/bus.js"));
    let store = report(&repo, "dispatch", Some("lib/store.js"));

    assert_site(
        &bus,
        "src/app.js",
        4,
        SiteLabel::Unresolved,
        "2 definitions",
    );
    assert_site(
        &store,
        "src/app.js",
        4,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

const JS_WORKSPACE: &[(&str, &str)] = &[
    (
        "packages/lib/package.json",
        r#"{
  "name": "lib",
  "exports": {
    ".": { "@lib/source": "./src/index.ts", "types": "./index.d.ts", "import": "./index.js" },
    "./v3": { "@lib/source": "./src/v3/index.ts" },
    "./sub/*": { "@lib/source": "./src/sub/*.ts" }
  }
}
"#,
    ),
    (
        "packages/lib/src/index.ts",
        "export * from \"./v4/index.js\";\nexport { make as build } from \"./v4/make.js\";\nexport { z, z as default };\nimport * as z from \"./v4/index.js\";\n",
    ),
    (
        "packages/lib/src/v4/index.ts",
        "export * from \"./external.js\";\nexport * as core from \"./core.js\";\n",
    ),
    (
        "packages/lib/src/v4/external.ts",
        "export * from \"./schemas.js\";\nexport * as util from \"./util.js\";\n",
    ),
    (
        "packages/lib/src/v4/core.ts",
        "export * as util from \"./util.js\";\n",
    ),
    (
        "packages/lib/src/v4/schemas.ts",
        "export function strictObject() {\n  return 1;\n}\n\nexport function boolean() {\n  return 2;\n}\n",
    ),
    (
        "packages/lib/src/v4/util.ts",
        "export function isPlainObject() {\n  return true;\n}\n",
    ),
    (
        "packages/lib/src/v4/make.ts",
        "export function make() {\n  return 1;\n}\n",
    ),
    (
        "packages/lib/src/v3/index.ts",
        "export * from \"./types.js\";\n",
    ),
    (
        "packages/lib/src/v3/types.ts",
        "export function strictObject() {\n  return 3;\n}\n\nexport function boolean() {\n  return 4;\n}\n\nexport function build() {\n  return 5;\n}\n\nexport function isPlainObject() {\n  return false;\n}\n",
    ),
    (
        "packages/lib/src/v3/extras.ts",
        "export function extra() {\n  return 7;\n}\n",
    ),
    (
        "packages/lib/src/sub/extras.ts",
        "export function extra() {\n  return 6;\n}\n",
    ),
    ("packages/app/package.json", "{\n  \"name\": \"app\"\n}\n"),
];

fn workspace_with(extra: &[(&str, &str)]) -> Repo {
    let files: Vec<(String, String)> = JS_WORKSPACE
        .iter()
        .chain(extra)
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    scanned_owned(&files)
}

#[test]
fn a_workspace_package_import_follows_exports_and_star_reexports_to_the_definition() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\nimport * as legacy from \"lib/v3\";\n\nexport const current = z.strictObject();\nexport const old = legacy.strictObject();\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let path = "packages/app/src/use.ts";
    let current = line_of(&repo, path, "z.strictObject()");
    assert_site(&report, path, current, SiteLabel::Proven, "import");
    let old = line_of(&repo, path, "legacy.strictObject()");
    assert_site(
        &report,
        path,
        old,
        SiteLabel::Excluded,
        "resolves to `strictObject` (packages/lib/src/v3/types.ts:1)",
    );
}

#[test]
fn a_chain_of_namespace_reexports_reaches_the_member_definition() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\n\nexport const deep = z.core.util.isPlainObject({});\nexport const shallow = z.util.isPlainObject({});\n",
    )]);
    let report = report(&repo, "isPlainObject", Some("packages/lib/src/v4/util.ts"));

    let path = "packages/app/src/use.ts";
    for needle in ["z.core.util.isPlainObject", "z.util.isPlainObject"] {
        let line = line_of(&repo, path, needle);
        assert_site(&report, path, line, SiteLabel::Proven, "import");
    }
}

#[test]
fn an_aliased_reexport_resolves_to_the_original_definition() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\n\nexport const built = z.build();\n",
    )]);
    let report = report(&repo, "build", None);

    let line = line_of(&repo, "packages/app/src/use.ts", "z.build()");
    assert_site(
        &report,
        "packages/app/src/use.ts",
        line,
        SiteLabel::Excluded,
        "resolves to `make` (packages/lib/src/v4/make.ts:1)",
    );
}

#[test]
fn a_default_import_and_a_renamed_named_import_follow_the_export_rows() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import def from \"lib\";\nimport { z as named } from \"lib\";\n\nexport const a = def.boolean();\nexport const b = named.boolean();\n",
    )]);
    let report = report(&repo, "boolean", Some("packages/lib/src/v4/schemas.ts"));

    let path = "packages/app/src/use.ts";
    for needle in ["def.boolean()", "named.boolean()"] {
        let line = line_of(&repo, path, needle);
        assert_site(&report, path, line, SiteLabel::Proven, "import");
    }
}

#[test]
fn a_package_that_imports_itself_by_name_is_followed() {
    let repo = workspace_with(&[(
        "packages/lib/src/self.ts",
        "import * as z from \"lib\";\n\nexport const s = z.strictObject();\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let line = line_of(&repo, "packages/lib/src/self.ts", "z.strictObject()");
    assert_site(
        &report,
        "packages/lib/src/self.ts",
        line,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn an_exports_pattern_resolves_the_subpath_to_its_file() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import * as s from \"lib/sub/extras\";\n\nexport const e = s.extra();\n",
    )]);
    let report = report(&repo, "extra", Some("packages/lib/src/sub/extras.ts"));

    let line = line_of(&repo, "packages/app/src/use.ts", "s.extra()");
    assert_site(
        &report,
        "packages/app/src/use.ts",
        line,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_relative_import_of_a_barrel_follows_its_star_reexports() {
    let repo = workspace_with(&[(
        "packages/lib/src/v4/tests/barrel.test.ts",
        "import * as z from \"../index.js\";\n\nexport const s = z.strictObject();\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let path = "packages/lib/src/v4/tests/barrel.test.ts";
    let line = line_of(&repo, path, "z.strictObject()");
    assert_site(&report, path, line, SiteLabel::Proven, "import");
}

#[test]
fn a_package_the_workspace_does_not_define_follows_the_plain_import_rule() {
    let repo = workspace_with(&[(
        "packages/app/src/ext.ts",
        "import * as pad from \"left-pad\";\nimport * as lib from \"left-pad/lib\";\n\nexport const e = pad.strictObject();\nexport const f = lib.strictObject();\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let path = "packages/app/src/ext.ts";
    assert_site(
        &report,
        path,
        line_of(&repo, path, "pad.strictObject()"),
        SiteLabel::Excluded,
        "receiver `pad` is imported from `left-pad`",
    );
    assert_ne!(
        site_at(&report, path, line_of(&repo, path, "lib.strictObject()")).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_tsconfig_alias_to_a_sibling_folder_keeps_the_proof_of_the_import() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"alias\"}\n"),
        (
            "config/tsconfig.json",
            "{ \"compilerOptions\": { \"baseUrl\": \"..\", \"paths\": { \"@app/*\": [\"src/*\"] } } }\n",
        ),
        (
            "src/main.ts",
            "import { helper } from \"@app/util\";\nexport function run() { return helper(); }\n",
        ),
        ("src/other.ts", "export function helper() { return 2; }\n"),
        ("src/util.ts", "export function helper() { return 1; }\n"),
    ]);
    let report = report(&repo, "helper", Some("src/util.ts"));

    assert_site(&report, "src/main.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_bundler_alias_to_a_sibling_folder_keeps_the_proof_of_the_import() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"alias\"}\n"),
        (
            "web/vite.config.js",
            "export default { resolve: { alias: { \"@components\": \"/src/lib\", $lib: \"/src/lib\" } } };\n",
        ),
        (
            "web/src/main.js",
            "import { thing } from \"$lib/thing\";\nimport { thing as other } from \"@components/thing2\";\nexport function run() { return thing(); }\nexport function run2() { return other(); }\n",
        ),
        (
            "web/src/lib/thing.js",
            "export function thing() { return 1; }\n",
        ),
        (
            "web/src/lib/thing2.js",
            "export function thing() { return 2; }\n",
        ),
    ]);
    let report = report(&repo, "thing", Some("web/src/lib/thing.js"));

    assert_site(&report, "web/src/main.js", 3, SiteLabel::Proven, "import");
}

#[test]
fn an_import_alias_the_index_cannot_resolve_is_never_excluded() {
    let repo = workspace_with(&[(
        "packages/app/src/alias.ts",
        "import * as a from \"#lib\";\nimport * as b from \"@/lib\";\nimport * as c from \"~/lib\";\n\nexport const x = a.strictObject();\nexport const y = b.strictObject();\nexport const w = c.strictObject();\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    for needle in ["a.strictObject()", "b.strictObject()", "c.strictObject()"] {
        let line = line_of(&repo, "packages/app/src/alias.ts", needle);
        assert_ne!(
            site_at(&report, "packages/app/src/alias.ts", line).label,
            SiteLabel::Excluded,
            "{needle}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_workspace_package_without_an_indexed_export_target_is_never_excluded() {
    let repo = workspace_with(&[
        (
            "packages/built/package.json",
            "{\n  \"name\": \"dist-only\",\n  \"exports\": { \".\": \"./dist/index.js\" }\n}\n",
        ),
        (
            "packages/app/src/built.ts",
            "import * as d from \"dist-only\";\n\nexport const e = d.strictObject();\n",
        ),
    ]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let line = line_of(&repo, "packages/app/src/built.ts", "d.strictObject()");
    assert_ne!(
        site_at(&report, "packages/app/src/built.ts", line).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_parameter_named_like_the_import_binding_turns_the_import_rule_off() {
    let repo = workspace_with(&[(
        "packages/app/src/shadow.ts",
        "import * as z from \"lib\";\n\nexport function wrap(z) {\n  return z.strictObject();\n}\n",
    )]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let line = line_of(&repo, "packages/app/src/shadow.ts", "z.strictObject()");
    assert_ne!(
        site_at(&report, "packages/app/src/shadow.ts", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_member_read_of_an_imported_object_keeps_its_object_member_label() {
    let repo = workspace_with(&[
        (
            "packages/lib/src/api.ts",
            "export const api = {\n  strictObject: () => 1,\n};\n",
        ),
        (
            "packages/app/src/members.ts",
            "import { api } from \"../../lib/src/api.js\";\n\nexport const m = api.strictObject();\n",
        ),
    ]);
    let report = report(
        &repo,
        "strictObject",
        Some("packages/lib/src/v4/schemas.ts"),
    );

    let line = line_of(&repo, "packages/app/src/members.ts", "api.strictObject()");
    assert_site(
        &report,
        "packages/app/src/members.ts",
        line,
        SiteLabel::Excluded,
        "resolves to `strictObject` (packages/lib/src/api.ts:2)",
    );
}

const JS_MERGED_TYPE: &[(&str, &str)] = &[
    (
        "packages/lib/package.json",
        "{\n  \"name\": \"lib\",\n  \"exports\": { \".\": \"./src/index.ts\" }\n}\n",
    ),
    (
        "packages/lib/src/index.ts",
        "export * from \"./in-out.js\";\n",
    ),
    (
        "packages/lib/src/in-out.ts",
        "export type output<T> = T;\n\nexport function output() {\n  return 1;\n}\n",
    ),
];

#[test]
fn a_declaration_merged_type_and_function_count_as_one_definition() {
    let mut files: Vec<(&str, &str)> = JS_MERGED_TYPE.to_vec();
    files.push((
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\n\nexport type A = z.output<number>;\nexport const b = z.output();\n",
    ));
    let repo = scanned_repo(&files);
    let report = first_definition_report(&repo, "output", "packages/lib/src/in-out.ts");

    let path = "packages/app/src/use.ts";
    for needle in ["z.output<number>", "z.output()"] {
        let line = line_of(&repo, path, needle);
        assert_site(&report, path, line, SiteLabel::Proven, "import");
    }
}

#[test]
fn a_bare_type_usage_is_shadowed_by_a_type_in_its_file_or_enclosing_function() {
    let mut files: Vec<(String, String)> = JS_MERGED_TYPE
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    for n in 1..=13 {
        files.push((
            format!("packages/lib/src/decoys/d{n:02}.ts"),
            "export type output<T> = T;\n".to_string(),
        ));
    }
    files.push((
        "packages/app/src/shadow-type.ts".to_string(),
        "import * as z from \"lib\";\n\ntype output<T> = T;\n\nexport type A = output<number>;\nexport type B = z.output<number>;\n\nexport function f() {\n  type output<T> = T[];\n  return null as output<string>;\n}\n".to_string(),
    ));
    let repo = scanned_owned(&files);
    let report = first_definition_report(&repo, "output", "packages/lib/src/in-out.ts");

    let path = "packages/app/src/shadow-type.ts";
    assert_site(
        &report,
        path,
        line_of(&repo, path, "A = output<number>"),
        SiteLabel::Excluded,
        "resolves to `output` (packages/app/src/shadow-type.ts:3)",
    );
    assert_site(
        &report,
        path,
        line_of(&repo, path, "as output<string>"),
        SiteLabel::Excluded,
        "resolves to `output` (packages/app/src/shadow-type.ts:9)",
    );
    assert_site(
        &report,
        path,
        line_of(&repo, path, "z.output<number>"),
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn an_excluded_use_names_the_definition_that_has_the_meaning_of_the_use() {
    let mut files: Vec<(&str, &str)> = JS_MERGED_TYPE.to_vec();
    files.push((
        "packages/other/src/box.ts",
        "export class Box {\n  output() {\n    return 1;\n  }\n}\n",
    ));
    files.push((
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\n\nexport type A = z.output<number>;\nexport const b = z.output();\n",
    ));
    let repo = scanned_repo(&files);
    let report = report(&repo, "output", Some("packages/other/src/box.ts"));

    let path = "packages/app/src/use.ts";
    assert_site(
        &report,
        path,
        line_of(&repo, path, "z.output<number>"),
        SiteLabel::Excluded,
        "resolves to `output` (packages/lib/src/in-out.ts:1)",
    );
    assert_site(
        &report,
        path,
        line_of(&repo, path, "z.output()"),
        SiteLabel::Excluded,
        "resolves to `output` (packages/lib/src/in-out.ts:3)",
    );
}

#[test]
fn an_explicit_export_hides_the_same_name_from_a_star_reexport() {
    let repo = scanned_repo(&[
        (
            "packages/lib/src/pick/index.ts",
            "export * from \"./left.js\";\nexport { pick } from \"./right.js\";\n",
        ),
        (
            "packages/lib/src/pick/left.ts",
            "export function pick() {\n  return 1;\n}\n",
        ),
        (
            "packages/lib/src/pick/right.ts",
            "export function pick() {\n  return 2;\n}\n",
        ),
        (
            "packages/app/src/pick.ts",
            "import * as p from \"../../lib/src/pick/index.js\";\n\nexport const v = p.pick();\n",
        ),
    ]);
    let report = report(&repo, "pick", Some("packages/lib/src/pick/right.ts"));

    let line = line_of(&repo, "packages/app/src/pick.ts", "p.pick()");
    assert_site(
        &report,
        "packages/app/src/pick.ts",
        line,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn star_reexports_that_form_a_cycle_leave_the_label_to_the_name() {
    let repo = scanned_repo(&[
        ("packages/lib/src/cycle/a.ts", "export * from \"./b.js\";\n"),
        ("packages/lib/src/cycle/b.ts", "export * from \"./a.js\";\n"),
        (
            "packages/lib/src/cycle/real.ts",
            "export function thing() {\n  return 1;\n}\n",
        ),
        (
            "packages/app/src/cycle.ts",
            "import * as c from \"../../lib/src/cycle/a.js\";\n\nexport const v = c.thing();\n",
        ),
    ]);
    let report = report(&repo, "thing", None);

    let line = line_of(&repo, "packages/app/src/cycle.ts", "c.thing()");
    assert_ne!(
        site_at(&report, "packages/app/src/cycle.ts", line).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_member_chain_is_not_excluded_by_an_import_of_its_last_name() {
    let repo = scanned_repo(&[
        (
            "lib/request.js",
            "var req = module.exports = {};\n\nreq.accepts = function () {\n  return 1;\n};\n",
        ),
        ("index.js", "exports.request = require('./lib/request');\n"),
        (
            "test/exports.js",
            "var express = require('../');\nvar request = require('left-pad');\n\nmodule.exports = typeof express.request.accepts;\n",
        ),
    ]);
    let report = report(&repo, "accepts", Some("lib/request.js"));

    let line = line_of(&repo, "test/exports.js", "express.request.accepts");
    assert_ne!(
        site_at(&report, "test/exports.js", line).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

const JS_OBJECT_LITERAL_HOLDERS: [&str; 6] = [
    "function render(x) {\n  return x;\n}\nexport const api = { render };\n",
    "function render(x) {\n  return x;\n}\nexport default { render };\n",
    "function render(x) {\n  return x;\n}\nconst api = { render };\nmodule.exports = api;\n",
    "function render(x) {\n  return x;\n}\nfunction make() {\n  return { render };\n}\nmodule.exports = make;\n",
    "function render(x) {\n  return x;\n}\nconst api = { key: render, other: 1 };\nmodule.exports = api;\n",
    "function render(x) {\n  return x;\n}\nconst api = {\n  render,\n  other: 1,\n};\nmodule.exports = api;\n",
];

#[test]
fn a_javascript_member_call_keeps_a_function_an_object_literal_holds_as_a_rival() {
    for util in JS_OBJECT_LITERAL_HOLDERS {
        let repo = scanned_repo(&[JS_VIEW, JS_MEMBER_CALL, ("lib/util.js", util)]);
        let report = report(&repo, "render", Some("lib/view.js"));

        let site = site_at(&report, "lib/use.js", 2);
        assert_eq!(
            (site.label, site.basis.as_str()),
            (SiteLabel::Unresolved, "2 definitions"),
            "{util}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_javascript_function_passed_as_an_argument_or_used_as_a_receiver_stays_free() {
    let repo = scanned_repo(&[
        JS_VIEW,
        JS_MEMBER_CALL,
        (
            "lib/util.js",
            "var render = make();\nregister('*', render);\nrender.use(1);\nmodule.exports = {};\n",
        ),
    ]);
    let report = report(&repo, "render", Some("lib/view.js"));

    assert_site(&report, "lib/use.js", 2, SiteLabel::Likely, "name only");
}

#[test]
fn a_java_call_keeps_a_record_component_as_a_rival() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    int size() {\n        return 1;\n    }\n}\n",
        ),
        (
            "q/R.java",
            "package q;\n\npublic record R(int size, int other) {\n    static int count = 0;\n}\n",
        ),
        (
            "q/B.java",
            "package q;\n\npublic class B {\n    int go() {\n        return make().size();\n    }\n\n    int tally() {\n        return make().count();\n    }\n}\n",
        ),
        (
            "p/C.java",
            "package p;\n\npublic class C {\n    int count() {\n        return 1;\n    }\n}\n",
        ),
    ]);
    let size = report(&repo, "size", Some("p/A.java"));
    let count = report(&repo, "count", Some("p/C.java"));

    let line = |needle: &str| line_of(&repo, "q/B.java", needle);
    assert_site(
        &size,
        "q/B.java",
        line("make().size()"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
    assert_site(
        &count,
        "q/B.java",
        line("make().count()"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_go_declaration_that_may_hold_a_func_stays_a_rival_of_a_call() {
    let repo = scanned_repo(&[
        (
            "lib/fns.go",
            "package lib\n\nfunc Hook() {}\n\nfunc Serve() {}\n\nfunc Later() {}\n\nfunc Plain() {}\n",
        ),
        (
            "store/decl.go",
            "package store\n\nimport \"net/http\"\n\nvar Hook func(n int)\n\nvar Later = makeHook()\n\nvar Plain int\n\ntype Server struct {\n\tServe http.HandlerFunc\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nfunc call(x any) {\n\tx.Hook()\n\tx.Serve()\n\tx.Later()\n\tx.Plain()\n}\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "use/use.go", needle);

    for name in ["Hook", "Serve", "Later"] {
        let report = report(&repo, name, Some("lib/fns.go"));
        assert_site(
            &report,
            "use/use.go",
            line(&format!("x.{name}()")),
            SiteLabel::Unresolved,
            "2 definitions",
        );
    }
    let plain = report(&repo, "Plain", Some("lib/fns.go"));
    assert_site(
        &plain,
        "use/use.go",
        line("x.Plain()"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_go_variable_named_with_a_letter_of_the_var_keyword_is_read_by_its_name() {
    let repo = scanned_repo(&[
        ("lib/fns.go", "package lib\n\nfunc v() {}\n\nfunc a() {}\n"),
        (
            "store/decl.go",
            "package store\n\nvar v = func() int { return 1 }\n\nvar a, r int\n",
        ),
        (
            "use/use.go",
            "package use\n\nfunc call(x any) {\n\tx.v()\n\tx.a()\n}\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "use/use.go", needle);

    let v = report(&repo, "v", Some("lib/fns.go"));
    assert_site(
        &v,
        "use/use.go",
        line("x.v()"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let a = report(&repo, "a", Some("lib/fns.go"));
    assert_site(
        &a,
        "use/use.go",
        line("x.a()"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_rust_static_of_a_function_type_stays_a_rival_of_a_call() {
    let repo = scanned_repo(&[
        (
            "src/run.rs",
            "pub fn handler(x: i32) -> i32 {\n    x\n}\n\npub fn label() -> i32 {\n    1\n}\n",
        ),
        (
            "src/table.rs",
            "pub static handler: fn(i32) -> i32 = double;\n\npub static label: &str = \"b\";\n\nfn double(x: i32) -> i32 {\n    x * 2\n}\n",
        ),
        (
            "src/use.rs",
            "fn go() {\n    handler(1);\n    label();\n}\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "src/use.rs", needle);

    let handler = report(&repo, "handler", Some("src/run.rs"));
    assert_site(
        &handler,
        "src/use.rs",
        line("handler(1)"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let label = report(&repo, "label", Some("src/run.rs"));
    assert_site(
        &label,
        "src/use.rs",
        line("label()"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_member_read_of_a_method_that_only_overloads_define_is_likely_not_unresolved() {
    let repo = scanned_repo(&[
        (
            "lib/view.ts",
            "export class View {\n  render(a: string): void;\n  render(a: number): void;\n  render(a: any): void {}\n}\n",
        ),
        (
            "lib/use.ts",
            "export function go(v) {\n  const f = v.render;\n  return f;\n}\n",
        ),
    ]);
    let report = first_definition_report(&repo, "render", "lib/view.ts");

    assert_site(
        &report,
        "lib/use.ts",
        2,
        SiteLabel::Likely,
        "receiver not typed",
    );
}

#[test]
fn a_declaration_file_does_not_take_an_import_from_the_script_next_to_it() {
    let repo = scanned_repo(&[
        ("lib.d.ts", "export declare function foo(): number;\n"),
        ("lib.js", "export function foo() { return 1; }\n"),
        (
            "main.ts",
            "import { foo } from \"./lib\";\nexport function run() { return foo(); }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("lib.js"));

    assert_site(&report, "main.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_javascript_importer_takes_the_javascript_twin_of_a_typescript_module() {
    let repo = scanned_repo(&[
        ("util.ts", "export function foo(): number { return 1; }\n"),
        ("util.js", "export function foo() { return 1; }\n"),
        (
            "main.js",
            "import { foo } from \"./util\";\nexport function run() { return foo(); }\n",
        ),
        (
            "main.ts",
            "import { foo } from \"./util\";\nexport function go() { return foo(); }\n",
        ),
    ]);

    let script = report(&repo, "foo", Some("util.js"));
    assert_site(&script, "main.js", 2, SiteLabel::Proven, "import");
    assert_eq!(
        site_at(&script, "main.ts", 2).label,
        SiteLabel::Excluded,
        "{}",
        dump(&script)
    );
    let source = report(&repo, "foo", Some("util.ts"));
    assert_site(&source, "main.ts", 2, SiteLabel::Proven, "import");
    assert_eq!(
        site_at(&source, "main.js", 2).label,
        SiteLabel::Excluded,
        "{}",
        dump(&source)
    );
}

#[test]
fn a_package_export_prefers_the_script_over_its_declaration_file() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        ("app/package.json", "{\"name\":\"app\"}\n"),
        (
            "app/main.ts",
            "import { foo } from \"lib2\";\nexport function run() { return foo(); }\n",
        ),
        (
            "packages/lib2/package.json",
            "{ \"name\": \"lib2\", \"exports\": { \".\": { \"types\": \"./index.d.ts\", \"default\": \"./index.js\" } } }\n",
        ),
        (
            "packages/lib2/index.d.ts",
            "export declare function foo(): number;\n",
        ),
        (
            "packages/lib2/index.js",
            "export function foo() { return 1; }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("packages/lib2/index.js"));

    assert_site(&report, "app/main.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_definition_only_a_declaration_file_holds_proves_nothing_for_another_definition() {
    let repo = scanned_repo(&[
        ("types/lib.d.ts", "export declare function foo(): number;\n"),
        ("src/real.ts", "export function foo() { return 1; }\n"),
        (
            "src/main.ts",
            "import { foo } from \"../types/lib\";\nexport function run() { return foo(); }\n",
        ),
    ]);

    let real = report(&repo, "foo", Some("src/real.ts"));
    assert_ne!(
        site_at(&real, "src/main.ts", 2).label,
        SiteLabel::Excluded,
        "{}",
        dump(&real)
    );
    let declared = report(&repo, "foo", Some("types/lib.d.ts"));
    assert_site(&declared, "src/main.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_call_after_a_line_that_ends_with_a_dot_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        ("barrel.ts", "export * from \"./m\";\n"),
        ("m.ts", "export function foo() { return 1; }\n"),
        (
            "other.ts",
            "export class K { foo() { return 3; } }\nexport function mk(): K { return new K(); }\n",
        ),
        (
            "use.ts",
            "import { foo } from \"./barrel\";\nimport { mk } from \"./other\";\nexport function i() { return mk().\n   foo(); }\nexport function j() { return mk()\n   .foo(); }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for line in [4, 6] {
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Excluded,
            "a call on a call result never reaches the top-level `foo`",
        );
    }
}

#[test]
fn an_import_named_like_a_path_keyword_prefix_is_still_a_package_outside_the_index() {
    let repo = scanned_repo(&[
        ("lib/agent.js", "export function get() {\n  return 1;\n}\n"),
        (
            "lib/use.js",
            "import * as supertest from \"supertest\";\nimport * as selfsigned from \"selfsigned\";\n\nexport const a = supertest.get();\nexport const b = selfsigned.get();\n",
        ),
    ]);
    let report = report(&repo, "get", Some("lib/agent.js"));

    assert_site(
        &report,
        "lib/use.js",
        4,
        SiteLabel::Excluded,
        "receiver `supertest` is imported from `supertest`",
    );
    assert_site(
        &report,
        "lib/use.js",
        5,
        SiteLabel::Excluded,
        "receiver `selfsigned` is imported from `selfsigned`",
    );
}

#[test]
fn the_import_resolver_reads_an_index_whose_paths_use_backslashes() {
    let repo = workspace_with(&[(
        "packages/app/src/use.ts",
        "import * as z from \"lib\";\n\nexport const deep = z.core.util.isPlainObject({});\nexport const shallow = z.util.isPlainObject({});\n",
    )]);
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'isPlainObject' AND path = 'packages/lib/src/v4/util.ts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    drop(conn);
    let writable = rusqlite::Connection::open(&repo.db).unwrap();
    for table in [
        "symbols",
        "files",
        "identifiers",
        "pending_relationships",
        "relationships",
    ] {
        writable
            .execute(
                &format!("UPDATE {table} SET path = replace(path, '/', char(92))"),
                [],
            )
            .unwrap();
    }
    drop(writable);
    let conn = open_read_only(&repo.db).unwrap();
    let target = get_symbol_by_id(&conn, &id).unwrap().unwrap();
    let report = reference_report(&conn, &repo.root, &target, false).unwrap();

    let path = "packages/app/src/use.ts";
    for needle in ["z.core.util.isPlainObject", "z.util.isPlainObject"] {
        let line = line_of(&repo, path, needle);
        assert_site(&report, path, line, SiteLabel::Proven, "import");
    }
}

#[test]
fn a_test_label_that_shares_the_name_of_an_import_does_not_shadow_it() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function mix(a: string) { return a }\n"),
        (
            "m.test.ts",
            "import { mix } from './m'\ndescribe('mix', () => {\n  it('works', () => {\n    expect(mix('a')).toBe('a')\n  })\n})\n",
        ),
        (
            "nested.test.ts",
            "import { mix } from './m'\ndescribe('outer', () => {\n  describe('mix', () => {\n    it('works', () => {\n      expect(mix('a')).toBe('a')\n    })\n  })\n})\n",
        ),
    ]);
    let report = report(&repo, "mix", Some("m.ts"));

    assert_site(&report, "m.test.ts", 4, SiteLabel::Proven, "import");
    assert_site(&report, "nested.test.ts", 5, SiteLabel::Proven, "import");
}

#[test]
fn a_package_whose_script_entry_is_a_shim_falls_back_to_its_declaration_entry() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        ("app/package.json", "{\"name\":\"app\"}\n"),
        (
            "app/main.ts",
            "import { foo } from \"@scope/lib\";\nexport function run() { return foo(); }\n",
        ),
        ("other/foo.ts", "export function foo() { return 2; }\n"),
        (
            "packages/lib/package.json",
            "{ \"name\": \"@scope/lib\", \"exports\": { \".\": { \"types\": \"./index.d.ts\", \"default\": \"./index.js\" } } }\n",
        ),
        (
            "packages/lib/index.d.ts",
            "export { foo } from \"./src/foo\";\n",
        ),
        (
            "packages/lib/index.js",
            "export * from \"./dist/entry.js\";\n",
        ),
        (
            "packages/lib/src/foo.ts",
            "export function foo() { return 1; }\n",
        ),
    ]);

    let real = report(&repo, "foo", Some("packages/lib/src/foo.ts"));
    assert_site(&real, "app/main.ts", 2, SiteLabel::Proven, "import");
    let other = report(&repo, "foo", Some("other/foo.ts"));
    assert_site(
        &other,
        "app/main.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/lib/src/foo.ts:1)",
    );
}

#[test]
fn a_call_after_a_line_that_ends_with_a_comment_or_a_signature_dot_is_a_bare_call() {
    let repo = scanned_repo(&[
        ("a.js", "export function helper() { return 1 }\n"),
        (
            "b.js",
            "import { helper } from './a.js'\nexport function use() {\n  const x = 1 // set x.\n  helper()\n  const y = 2 // Done.\n  return helper()\n}\n",
        ),
        ("a.py", "def helper():\n    return 1\n"),
        (
            "b.py",
            "from a import helper\n\n\nclass P:\n    def m(self) -> int: ...\n\n\nhelper()\n\n\ndef use():\n    x = 1  # set x.\n    helper()\n",
        ),
        ("a.ts", "export function helper() { return 1 }\n"),
        (
            "b.ts",
            "import { helper } from './a'\nconst items = [...\n]\nexport const x = 1 // eslint-disable-line no-console.\nhelper()\n",
        ),
    ]);

    let js = report(&repo, "helper", Some("a.js"));
    assert_site(&js, "b.js", 4, SiteLabel::Proven, "import");
    let py = report(&repo, "helper", Some("a.py"));
    assert_site(&py, "b.py", 8, SiteLabel::Proven, "import");
    assert_site(&py, "b.py", 13, SiteLabel::Proven, "import");
    let ts = report(&repo, "helper", Some("a.ts"));
    assert_site(&ts, "b.ts", 5, SiteLabel::Proven, "import");
}

#[test]
fn a_call_after_a_line_that_ends_with_an_optional_or_non_null_member_dot_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        ("m.ts", "export function foo() { return 1; }\n"),
        (
            "other.ts",
            "export class K { foo() { return 3; } }\nexport function mk(): K | undefined { return new K(); }\n",
        ),
        (
            "use.ts",
            "import { foo } from \"./m\";\nimport { mk } from \"./other\";\nexport function i() { return mk()?.\n   foo(); }\nexport function j() { return mk()!.\n   foo(); }\nexport function k(xs: any[]) { return xs[0]?.\n   foo(); }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for line in [4, 6, 8] {
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Excluded,
            "a call on a call result never reaches the top-level `foo`",
        );
    }
}

#[test]
fn a_java_record_with_an_annotation_before_it_keeps_its_component_as_a_rival() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    int x() {\n        return 1;\n    }\n}\n",
        ),
        (
            "q/B.java",
            "package q;\n\npublic class B {\n    int go() {\n        return make().x();\n    }\n}\n",
        ),
        (
            "q/P.java",
            "package q;\n\n@Deprecated(since = \"1\")\npublic record P(int x, int y) {\n}\n",
        ),
    ]);
    let report = report(&repo, "x", Some("p/A.java"));

    assert_site(
        &report,
        "q/B.java",
        5,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_go_field_of_an_alias_or_generic_func_type_stays_a_rival_of_a_call() {
    for declaration in [
        "type HF = func(int)\n\ntype S struct {\n\tHook HF\n}\n",
        "type F[T any] func(T)\n\ntype S struct {\n\tHook F[int]\n}\n",
    ] {
        let repo = scanned_repo(&[
            ("a/hook.go", "package a\n\nfunc Hook() {}\n"),
            ("b/h.go", &format!("package b\n\n{declaration}")),
            (
                "c/use.go",
                "package c\n\nfunc call(t any) {\n\tt.Hook()\n}\n",
            ),
        ]);
        let report = report(&repo, "Hook", Some("a/hook.go"));

        let site = site_at(&report, "c/use.go", 4);
        assert_eq!(
            (site.label, site.basis.as_str()),
            (SiteLabel::Unresolved, "2 definitions"),
            "{declaration}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_rust_static_of_a_function_type_alias_stays_a_rival_of_a_call() {
    let repo = scanned_repo(&[
        (
            "a/lib.rs",
            "pub fn HOOK() {}\n\npub fn SPACED() {}\n\npub fn PLAIN() {}\n",
        ),
        (
            "b/h.rs",
            "type Cb = fn(i32);\n\npub static HOOK: Cb = identity;\n\npub static SPACED: fn (i32) = identity;\n\ntype Count = u32;\n\npub static PLAIN: Count = 1;\n",
        ),
        (
            "c/use.rs",
            "pub fn call() {\n    HOOK();\n    SPACED();\n    PLAIN();\n}\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "c/use.rs", needle);

    for name in ["HOOK", "SPACED"] {
        let report = report(&repo, name, Some("a/lib.rs"));
        assert_site(
            &report,
            "c/use.rs",
            line(&format!("{name}()")),
            SiteLabel::Unresolved,
            "2 definitions",
        );
    }
    let plain = report(&repo, "PLAIN", Some("a/lib.rs"));
    assert_site(
        &plain,
        "c/use.rs",
        line("PLAIN()"),
        SiteLabel::Likely,
        "name only",
    );
}

#[test]
fn a_comment_beside_a_shorthand_property_does_not_hide_the_object_value() {
    for util in [
        "function render(x) {\n  return x;\n}\nconst api = {\n  render // the renderer\n};\nmodule.exports = api;\n",
        "function render(x) {\n  return x;\n}\nconst api = { render /* x */ };\nmodule.exports = api;\n",
        "function render(x) {\n  return x;\n}\nconst api = { /* x */ render, other: 1 };\nmodule.exports = api;\n",
        "function render(x) {\n  return x;\n}\nconst api = {\n  other: 1, // first\n  render, /* second */\n};\nmodule.exports = api;\n",
    ] {
        let repo = scanned_repo(&[JS_VIEW, JS_MEMBER_CALL, ("lib/util.js", util)]);
        let report = report(&repo, "render", Some("lib/view.js"));

        let site = site_at(&report, "lib/use.js", 2);
        assert_eq!(
            (site.label, site.basis.as_str()),
            (SiteLabel::Unresolved, "2 definitions"),
            "{util}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_declaration_file_and_the_script_with_its_stem_describe_one_module() {
    for (declaration, script) in [
        ("lib.d.ts", "lib.js"),
        ("lib.d.ts", "lib.ts"),
        ("lib.d.mts", "lib.mjs"),
        ("lib.d.cts", "lib.cjs"),
    ] {
        let repo = scanned_repo(&[
            (declaration, "export declare function foo(): number;\n"),
            (script, "export function foo() { return 1; }\n"),
            (
                "main.ts",
                "import { foo } from \"./lib\";\nexport function run() { return foo(); }\n",
            ),
        ]);

        for target in [declaration, script] {
            let report = report(&repo, "foo", Some(target));
            assert_site(&report, "main.ts", 2, SiteLabel::Proven, "import");
        }
    }
}

#[test]
fn a_package_that_names_only_a_declaration_entry_proves_the_script_beside_it() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        ("app/package.json", "{\"name\":\"app\"}\n"),
        (
            "app/main.ts",
            "import { foo } from \"lib3\";\nexport function run() { return foo(); }\n",
        ),
        (
            "packages/lib3/package.json",
            "{ \"name\": \"lib3\", \"exports\": { \".\": { \"types\": \"./index.d.ts\" } } }\n",
        ),
        (
            "packages/lib3/index.d.ts",
            "export declare function foo(): number;\n",
        ),
        (
            "packages/lib3/index.js",
            "export function foo() { return 1; }\n",
        ),
        ("other/foo.ts", "export function foo() { return 2; }\n"),
    ]);

    let script = report(&repo, "foo", Some("packages/lib3/index.js"));
    assert_site(&script, "app/main.ts", 2, SiteLabel::Proven, "import");
    let other = report(&repo, "foo", Some("other/foo.ts"));
    assert_ne!(
        site_at(&other, "app/main.ts", 2).label,
        SiteLabel::Proven,
        "{}",
        dump(&other)
    );
}

const DUAL_PACKAGE: &[(&str, &str)] = &[
    ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
    ("app/package.json", "{\"name\":\"app\"}\n"),
    (
        "app/a.cjs",
        "const { foo } = require('dual')\nmodule.exports = function run() { return foo() }\n",
    ),
    (
        "app/b.mjs",
        "import { foo } from 'dual'\nexport function run2() { return foo() }\n",
    ),
    (
        "packages/dual/cjs/index.js",
        "function foo() { return 1 }\nmodule.exports = { foo }\n",
    ),
    (
        "packages/dual/esm/index.js",
        "export function foo() { return 1 }\n",
    ),
];

#[test]
fn a_require_call_follows_the_require_condition_and_an_import_follows_the_import_condition() {
    let mut files = DUAL_PACKAGE.to_vec();
    files.push((
        "packages/dual/package.json",
        "{\"name\":\"dual\",\"exports\":{\".\":{\"import\":\"./esm/index.js\",\"require\":\"./cjs/index.js\"}}}\n",
    ));
    let repo = scanned_repo(&files);

    let esm = report(&repo, "foo", Some("packages/dual/esm/index.js"));
    assert_site(&esm, "app/b.mjs", 2, SiteLabel::Proven, "import");
    assert_ne!(
        site_at(&esm, "app/a.cjs", 2).label,
        SiteLabel::Proven,
        "{}",
        dump(&esm)
    );
    let cjs = report(&repo, "foo", Some("packages/dual/cjs/index.js"));
    assert_ne!(
        site_at(&cjs, "app/a.cjs", 2).label,
        SiteLabel::Excluded,
        "{}",
        dump(&cjs)
    );
    assert_site(
        &cjs,
        "app/b.mjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/dual/esm/index.js:1)",
    );
}

#[test]
fn a_package_with_two_scripts_and_no_kind_to_choose_by_is_left_to_the_name() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        ("app/package.json", "{\"name\":\"app\"}\n"),
        (
            "app/b.mjs",
            "import { foo } from 'dual'\nexport function run2() { return foo() }\n",
        ),
        (
            "packages/dual/package.json",
            "{\"name\":\"dual\",\"exports\":{\".\":{\"node\":\"./node/index.js\",\"default\":\"./web/index.js\"}}}\n",
        ),
        (
            "packages/dual/node/index.js",
            "export function foo() { return 1 }\n",
        ),
        (
            "packages/dual/web/index.js",
            "export function foo() { return 1 }\n",
        ),
    ]);

    for target in ["packages/dual/node/index.js", "packages/dual/web/index.js"] {
        let report = report(&repo, "foo", Some(target));
        let label = site_at(&report, "app/b.mjs", 2).label;
        assert!(
            label != SiteLabel::Proven && label != SiteLabel::Excluded,
            "{target}\n{}",
            dump(&report)
        );
    }
}

const SHARED_NAME_CONFLICT: &[(&str, &str)] = &[
    ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
    (
        "src/app.ts",
        "import { helper } from 'shared/util'\nexport function run() { return helper() }\n",
    ),
    (
        "src/shared/util.ts",
        "export function helper() { return 1 }\n",
    ),
    ("tools/shared/package.json", "{\"name\":\"shared\"}\n"),
    (
        "tools/shared/util.ts",
        "export function helper() { return 2 }\n",
    ),
];

fn shared_name_conflict_with(tsconfig: &[(&str, &str)]) -> Repo {
    let files: Vec<(&str, &str)> = SHARED_NAME_CONFLICT
        .iter()
        .chain(tsconfig)
        .copied()
        .collect();
    scanned_repo(&files)
}

#[test]
fn a_tsconfig_paths_alias_wins_over_a_workspace_package_of_the_same_prefix() {
    let repo = shared_name_conflict_with(&[(
        "tsconfig.json",
        "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"shared/*\":[\"src/shared/*\"]}}}\n",
    )]);

    let aliased = report(&repo, "helper", Some("src/shared/util.ts"));
    assert_site(&aliased, "src/app.ts", 2, SiteLabel::Proven, "import");
    let package = report(&repo, "helper", Some("tools/shared/util.ts"));
    assert_site(
        &package,
        "src/app.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (src/shared/util.ts:1)",
    );
}

#[test]
fn a_tsconfig_with_comments_trailing_commas_and_an_extends_chain_is_read_leniently() {
    let repo = shared_name_conflict_with(&[
        (
            "tsconfig.json",
            "{\n  // the project config\n  \"extends\": \"./config/base\",\n  \"compilerOptions\": { /* none here */ },\n}\n",
        ),
        (
            "config/base.json",
            "{\n  \"compilerOptions\": {\n    \"paths\": {\n      \"shared/*\": [\"../src/shared/*\",],\n    },\n  },\n}\n",
        ),
    ]);

    let aliased = report(&repo, "helper", Some("src/shared/util.ts"));
    assert_site(&aliased, "src/app.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn the_nearest_tsconfig_decides_and_a_far_one_does_not() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "tsconfig.json",
            "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"src/shared/*\"]}}}\n",
        ),
        (
            "app/tsconfig.json",
            "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"lib/shared/*\"]}}}\n",
        ),
        (
            "app/main.ts",
            "import { helper } from 'shared/util'\nexport function run() { return helper() }\n",
        ),
        (
            "src/shared/util.ts",
            "export function helper() { return 1 }\n",
        ),
        (
            "app/lib/shared/util.ts",
            "export function helper() { return 2 }\n",
        ),
    ]);

    let near = report(&repo, "helper", Some("app/lib/shared/util.ts"));
    assert_site(&near, "app/main.ts", 2, SiteLabel::Proven, "import");
    let far = report(&repo, "helper", Some("src/shared/util.ts"));
    assert_site(
        &far,
        "app/main.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (app/lib/shared/util.ts:1)",
    );
}

#[test]
fn a_base_url_that_names_the_file_wins_over_a_workspace_package() {
    let repo = shared_name_conflict_with(&[(
        "tsconfig.json",
        "{\"compilerOptions\":{\"baseUrl\":\"src\"}}\n",
    )]);

    let aliased = report(&repo, "helper", Some("src/shared/util.ts"));
    assert_site(&aliased, "src/app.ts", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_paths_key_that_matches_an_unindexed_file_on_disk_leaves_the_module_unknown() {
    let repo = shared_name_conflict_with(&[(
        "tsconfig.json",
        "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"gone/*\"]}}}\n",
    )]);
    fs::create_dir_all(repo.root.join("gone")).unwrap();
    fs::write(repo.root.join("gone/util.ts"), "export const unseen = 1;\n").unwrap();

    for target in ["src/shared/util.ts", "tools/shared/util.ts"] {
        let report = report(&repo, "helper", Some(target));
        assert_ne!(
            site_at(&report, "src/app.ts", 2).label,
            SiteLabel::Excluded,
            "{target}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_bare_named_import_from_a_node_builtin_is_excluded_even_beside_a_folder_of_that_name() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        (
            "src/fs/read.ts",
            "export function readFile() { return 1 }\n",
        ),
        (
            "src/fs/other.ts",
            "export function readFile() { return 2 }\n",
        ),
        (
            "src/use.ts",
            "import { readFile } from 'node:fs'\nexport function c() { return readFile() }\n",
        ),
    ]);
    let report = report(&repo, "readFile", Some("src/fs/read.ts"));

    assert_site(
        &report,
        "src/use.ts",
        2,
        SiteLabel::Excluded,
        "receiver `readFile` is imported from `node:fs`",
    );
}

fn binding_note(line: usize) -> String {
    format!(
        "import; line {line} binds the name as a callback parameter, loop variable, or catch parameter"
    )
}

#[test]
fn a_binding_with_a_block_body_excludes_the_read_and_one_without_downgrades_the_proof() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            "import { foo } from './m';\nexport function a(xs: any[]) { return xs.map(foo => foo()); }\nexport function b(xs: any[]) { return xs.map((n: number, foo: any) => foo()); }\nexport function c(xs: any[]) { return xs.map(({ foo }) => foo()); }\nexport function d(xs: any[]) { xs.forEach(function (foo) { foo() }) }\nexport function e(list: any[]) { for (const foo of list) foo() }\nexport function f(list: any) { for (const foo in list) { foo() } }\nexport function g(list: any[]) { for (const [k, foo] of list) { foo() } }\nexport function h() { try { x() } catch (foo) { foo() } }\nexport function i() { try { x() } catch ({ foo }) { foo() } }\nexport function j(xs: any[]) { xs.map(async (foo = 1) => foo()) }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for line in [2, 3, 4, 6, 11] {
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Likely,
            &binding_note(line),
        );
    }
    for line in [5, 7, 8, 9, 10] {
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Excluded,
            &binding_note(line).replacen("import; ", "", 1),
        );
    }
}

#[test]
fn a_name_that_is_only_an_argument_or_a_default_value_keeps_its_proof() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            "import { foo } from './m';\nexport function a(xs: any[]) { xs.map(foo); return foo(); }\nexport function b(xs: any[]) { xs.map((n = foo) => n); return foo(); }\nexport function c(xs: any[]) { for (const n of xs.map(foo)) n; return foo(); }\nexport function d(xs: any[]) { xs.map(({ foo: k }) => k); return xs.map(n => foo(n)); }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for line in 2..=4 {
        assert_site(&report, "use.ts", line, SiteLabel::Proven, "import");
    }
    assert_site(&report, "use.ts", 5, SiteLabel::Proven, "import");
}

#[test]
fn a_binding_in_an_earlier_caller_does_not_downgrade_the_proof_of_a_later_one() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            "import { foo } from './m';\nexport function a(xs: any[]) { return xs.map(foo => foo()); }\nexport function b() { return foo(); }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    assert_site(&report, "use.ts", 2, SiteLabel::Likely, &binding_note(2));
    assert_site(&report, "use.ts", 3, SiteLabel::Proven, "import");
}

#[test]
fn a_callback_parameter_never_excludes_the_site_for_the_other_definition() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        ("n.ts", "export function foo() { return 2 }\n"),
        (
            "use.ts",
            "import { foo } from './m';\nexport function a(xs: any[]) { return xs.map(foo => foo()); }\n",
        ),
    ]);

    let other = report(&repo, "foo", Some("n.ts"));
    assert_ne!(
        site_at(&other, "use.ts", 2).label,
        SiteLabel::Proven,
        "{}",
        dump(&other)
    );
    let imported = report(&repo, "foo", Some("m.ts"));
    assert_site(&imported, "use.ts", 2, SiteLabel::Likely, &binding_note(2));
}

#[test]
fn a_callback_parameter_with_a_declared_type_keeps_the_receiver_type_proof() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        (
            "def.ts",
            "export interface Def { tx(value: number): number }\n",
        ),
        (
            "use.ts",
            "import type { Def } from './def';\nexport const run = (inst: object, def: Def) => {\n  return def.tx(1);\n};\nexport const wrap = (def: Def) => def.tx(2);\n",
        ),
    ]);
    let report = report(&repo, "tx", Some("def.ts"));

    assert_site(&report, "use.ts", 3, SiteLabel::Proven, "receiver type");
    assert_site(&report, "use.ts", 5, SiteLabel::Proven, "receiver type");
}

const KERNEL_FILES: &[(&str, &str)] = &[
    ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
    (
        "app/main.js",
        "import { foo } from '@x/pkg'\nexport function run() { return foo() }\n",
    ),
    (
        "packages/kernel/index.js",
        "export * from './lib/entry.js'\n",
    ),
    (
        "packages/kernel/lib/entry.js",
        "export function foo() { return 1 }\n",
    ),
];

fn kernel_with(manifest: &str, extra: &[(&str, &str)]) -> Repo {
    let files: Vec<(&str, &str)> = KERNEL_FILES
        .iter()
        .chain(extra)
        .copied()
        .chain([("packages/kernel/package.json", manifest)])
        .collect();
    scanned_repo(&files)
}

#[test]
fn a_main_entry_beside_an_index_file_is_the_only_script_of_the_package() {
    let manifest = "{\"name\":\"@x/pkg\",\"main\":\"lib/entry.js\"}\n";

    let alone = kernel_with(manifest, &[]);
    let report_alone = report(&alone, "foo", Some("packages/kernel/lib/entry.js"));
    assert_site(&report_alone, "app/main.js", 2, SiteLabel::Proven, "import");

    let rivalled = kernel_with(
        manifest,
        &[(
            "packages/other/foo.js",
            "export function foo() { return 2 }\n",
        )],
    );
    let entry = report(&rivalled, "foo", Some("packages/kernel/lib/entry.js"));
    assert_site(&entry, "app/main.js", 2, SiteLabel::Proven, "import");
    let other = report(&rivalled, "foo", Some("packages/other/foo.js"));
    assert_site(
        &other,
        "app/main.js",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/kernel/lib/entry.js:1)",
    );
}

#[test]
fn a_types_entry_never_counts_as_a_second_script() {
    let types_files = [("packages/kernel/src/types.ts", "export type Id = number;\n")];
    for manifest in [
        "{\"name\":\"@x/pkg\",\"main\":\"lib/entry.js\",\"types\":\"src/types.ts\"}\n",
        "{\"name\":\"@x/pkg\",\"exports\":{\".\":{\"types\":\"./src/types.ts\",\"default\":\"./lib/entry.js\"}}}\n",
    ] {
        let repo = kernel_with(manifest, &types_files);
        let report = report(&repo, "foo", Some("packages/kernel/lib/entry.js"));

        assert_site(&report, "app/main.js", 2, SiteLabel::Proven, "import");
    }
}

#[test]
fn an_es_import_follows_the_module_entry_and_a_require_follows_main() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        ("app/package.json", "{\"name\":\"app\"}\n"),
        (
            "app/a.cjs",
            "const { foo } = require('dual')\nmodule.exports = function run() { return foo() }\n",
        ),
        (
            "app/b.mjs",
            "import { foo } from 'dual'\nexport function run2() { return foo() }\n",
        ),
        (
            "packages/dual/package.json",
            "{\"name\":\"dual\",\"module\":\"esm/index.js\",\"main\":\"cjs/index.js\"}\n",
        ),
        (
            "packages/dual/cjs/index.js",
            "export function foo() { return 1 }\n",
        ),
        (
            "packages/dual/esm/index.js",
            "export function foo() { return 2 }\n",
        ),
    ]);

    let esm = report(&repo, "foo", Some("packages/dual/esm/index.js"));
    assert_site(&esm, "app/b.mjs", 2, SiteLabel::Proven, "import");
    assert_site(
        &esm,
        "app/a.cjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/dual/cjs/index.js:1)",
    );
    let cjs = report(&repo, "foo", Some("packages/dual/cjs/index.js"));
    assert_site(&cjs, "app/a.cjs", 2, SiteLabel::Proven, "import");
    assert_site(
        &cjs,
        "app/b.mjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/dual/esm/index.js:1)",
    );
}

const PATHS_TO_SRC: &str =
    "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"shared/*\":[\"src/shared/*\"]}}}\n";

fn assert_src_helper_wins(repo: &Repo) {
    let aliased = report(repo, "helper", Some("src/shared/util.ts"));
    assert_site(&aliased, "src/app.ts", 2, SiteLabel::Proven, "import");
    let package = report(repo, "helper", Some("tools/shared/util.ts"));
    assert_site(
        &package,
        "src/app.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (src/shared/util.ts:1)",
    );
}

#[test]
fn a_tsconfig_with_a_byte_order_mark_is_read() {
    let with_mark = format!("\u{feff}{PATHS_TO_SRC}");
    let repo = shared_name_conflict_with(&[("tsconfig.json", &with_mark)]);

    assert_src_helper_wins(&repo);
}

#[test]
fn a_tsconfig_extends_array_is_followed_in_order_and_the_last_paths_win() {
    let repo = shared_name_conflict_with(&[
        (
            "tsconfig.json",
            "{\"extends\":[\"./a.json\",\"./b.json\"]}\n",
        ),
        (
            "a.json",
            "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"shared/*\":[\"tools/shared/*\"]}}}\n",
        ),
        (
            "b.json",
            "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"src/shared/*\"]}}}\n",
        ),
    ]);

    assert_src_helper_wins(&repo);
}

#[test]
fn a_tsconfig_that_cannot_be_read_leaves_a_workspace_package_unknown() {
    let repo = shared_name_conflict_with(&[("tsconfig.json", "{\"compilerOptions\":")]);

    for target in ["src/shared/util.ts", "tools/shared/util.ts"] {
        let report = report(&repo, "helper", Some(target));
        let label = site_at(&report, "src/app.ts", 2).label;
        assert!(
            label != SiteLabel::Proven && label != SiteLabel::Excluded,
            "{target}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_paths_key_that_matches_no_file_on_disk_falls_back_to_the_workspace_package() {
    let repo = shared_name_conflict_with(&[(
        "tsconfig.json",
        "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"gone/*\"]}}}\n",
    )]);

    let package = report(&repo, "helper", Some("tools/shared/util.ts"));
    assert_site(&package, "src/app.ts", 2, SiteLabel::Proven, "import");
    let other = report(&repo, "helper", Some("src/shared/util.ts"));
    assert_site(
        &other,
        "src/app.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (tools/shared/util.ts:1)",
    );
}

#[test]
fn a_catch_all_paths_key_does_not_stop_an_external_package_from_being_excluded() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "tsconfig.json",
            "{\"compilerOptions\":{\"baseUrl\":\".\",\"paths\":{\"*\":[\"node_modules/*\",\"src/types/*\"]}}}\n",
        ),
        (
            "lib/mine.ts",
            "export function debounce(f: () => void) { return f }\n",
        ),
        (
            "src/app.ts",
            "import { debounce } from 'lodash'\nexport function run() { return debounce(() => 1) }\n",
        ),
    ]);
    let report = report(&repo, "debounce", Some("lib/mine.ts"));

    assert_site(
        &report,
        "src/app.ts",
        2,
        SiteLabel::Excluded,
        "receiver `debounce` is imported from `lodash`",
    );
}

#[test]
fn a_named_import_from_a_node_builtin_subpath_is_excluded_beside_a_folder_of_its_last_segment() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        (
            "src/promises/read.ts",
            "export function readFile() { return 1 }\n",
        ),
        (
            "src/other/read.ts",
            "export function readFile() { return 2 }\n",
        ),
        (
            "src/use.ts",
            "import { readFile } from 'node:fs/promises'\nexport function c() { return readFile() }\n",
        ),
    ]);
    for target in ["src/other/read.ts", "src/promises/read.ts"] {
        let report = report(&repo, "readFile", Some(target));

        assert_site(
            &report,
            "src/use.ts",
            2,
            SiteLabel::Excluded,
            "receiver `readFile` is imported from `node:fs/promises`",
        );
    }
}

#[test]
fn a_bare_import_of_a_node_builtin_subpath_is_excluded_beside_a_folder_of_its_last_segment() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        (
            "src/promises/read.ts",
            "export function readFile() { return 1 }\n",
        ),
        (
            "src/use.ts",
            "import { readFile } from 'fs/promises'\nexport function c() { return readFile() }\n",
        ),
    ]);
    let report = report(&repo, "readFile", Some("src/promises/read.ts"));

    assert_site(
        &report,
        "src/use.ts",
        2,
        SiteLabel::Excluded,
        "receiver `readFile` is imported from `fs/promises`",
    );
}

#[test]
fn a_promise_catch_method_is_not_a_catch_clause() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("h.js", "export function handle(e) { return e }\n"),
        (
            "use.js",
            "import { handle } from './h.js';\nexport async function a(p) {\n  await p.catch(handle);\n  return handle(1);\n}\nexport async function b(p) {\n  await p?.catch(handle);\n  return handle(2);\n}\nexport async function c(p) {\n  try { await p } catch (handle) { handle(3) }\n}\n",
        ),
    ]);
    let report = report(&repo, "handle", Some("h.js"));

    assert_site(&report, "use.js", 4, SiteLabel::Proven, "import");
    assert_site(&report, "use.js", 8, SiteLabel::Proven, "import");
    assert_site(
        &report,
        "use.js",
        11,
        SiteLabel::Excluded,
        &binding_note(11).replacen("import; ", "", 1),
    );
}

#[test]
fn names_in_grouping_parentheses_of_a_ternary_are_not_arrow_parameters() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("spinner.tsx", "export function Spinner() { return null }\n"),
        (
            "view.tsx",
            "import { Spinner } from './spinner';\nexport function View({ items, loading }: any) {\n  return (\n    <div>\n      {loading ? (\n        <Spinner />\n      ) : items.map((item: any) => (\n        <span>{item}</span>\n      ))}\n      <Spinner />\n    </div>\n  );\n}\nexport const w = (xs: any[]) => xs.map((Spinner): number => Spinner());\n",
        ),
    ]);
    let report = report(&repo, "Spinner", Some("spinner.tsx"));

    assert_site(&report, "view.tsx", 6, SiteLabel::Proven, "import");
    assert_site(&report, "view.tsx", 10, SiteLabel::Proven, "import");
    assert_site(
        &report,
        "view.tsx",
        14,
        SiteLabel::Likely,
        &binding_note(14),
    );
}

#[test]
fn a_callback_parameter_hides_a_qualified_path_proof_too() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("zmod.ts", "export const o = { g() { return 2 } };\n"),
        (
            "use.ts",
            "import { o } from './zmod';\nexport function a(xs: any[]) { return xs.map(o => o.g()); }\nexport function b() { return o.g(); }\n",
        ),
    ]);
    let report = report(&repo, "g", Some("zmod.ts"));

    assert_site(
        &report,
        "use.ts",
        2,
        SiteLabel::Likely,
        "qualified path; line 2 binds the name as a callback parameter, loop variable, or catch parameter",
    );
    assert_site(&report, "use.ts", 3, SiteLabel::Proven, "qualified path");
}

#[test]
fn a_call_on_a_string_literal_is_not_a_bare_call() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("a.js", "export function trim() { return 1 }\n"),
        (
            "b.js",
            "import { trim } from './a.js'\nexport function use() {\n  'abc'.trim()\n  `x`.\n    trim()\n  \"abc\".\n    trim()\n  return trim()\n}\n",
        ),
    ]);
    let report = report(&repo, "trim", Some("a.js"));

    for line in [3, 5, 7] {
        assert_site(
            &report,
            "b.js",
            line,
            SiteLabel::Excluded,
            "a call on a call result never reaches the top-level `trim`",
        );
    }
    assert_site(&report, "b.js", 8, SiteLabel::Proven, "import");
}

#[test]
fn a_record_word_inside_an_annotation_string_does_not_make_a_class_a_record() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    int price() {\n        return 1;\n    }\n}\n",
        ),
        (
            "q/B.java",
            "package q;\n\npublic class B {\n    int go() {\n        return make().price();\n    }\n}\n",
        ),
        (
            "q/Trade.java",
            "package q;\n\n@Schema(description = \"A record of the trade\")\npublic class Trade {\n    private int price;\n}\n",
        ),
    ]);
    let report = report(&repo, "price", Some("p/A.java"));

    assert_site(&report, "q/B.java", 5, SiteLabel::Likely, "name only");
}

#[test]
fn a_require_of_a_package_with_only_a_module_field_reads_the_index_file() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "app/a.cjs",
            "const { foo } = require('modonly')\nmodule.exports = function run() { return foo() }\n",
        ),
        (
            "app/b.mjs",
            "import { foo } from 'modonly'\nexport function run2() { return foo() }\n",
        ),
        (
            "packages/modonly/package.json",
            "{\"name\":\"modonly\",\"module\":\"esm/index.js\"}\n",
        ),
        (
            "packages/modonly/index.js",
            "exports.foo = function foo() { return 2 }\n",
        ),
        (
            "packages/modonly/esm/index.js",
            "export function foo() { return 1 }\n",
        ),
    ]);

    let index = report(&repo, "foo", Some("packages/modonly/index.js"));
    assert_site(&index, "app/a.cjs", 2, SiteLabel::Proven, "import");
    assert_site(
        &index,
        "app/b.mjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/modonly/esm/index.js:1)",
    );
    let esm = report(&repo, "foo", Some("packages/modonly/esm/index.js"));
    assert_site(&esm, "app/b.mjs", 2, SiteLabel::Proven, "import");
    assert_site(
        &esm,
        "app/a.cjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/modonly/index.js:1)",
    );
}

#[test]
fn a_types_entry_in_another_folder_never_hides_the_main_script() {
    let repo = kernel_with(
        "{\"name\":\"@x/pkg\",\"main\":\"lib/entry.js\",\"types\":\"types/index.d.ts\"}\n",
        &[(
            "packages/kernel/types/index.d.ts",
            "export declare function foo(): number;\n",
        )],
    );
    let report = report(&repo, "foo", Some("packages/kernel/lib/entry.js"));

    assert_site(&report, "app/main.js", 2, SiteLabel::Proven, "import");
}

#[test]
fn a_package_with_only_a_types_entry_runs_its_index_file() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "app/use.js",
            "import { foo } from 'tonly'\nexport function run() { return foo() }\n",
        ),
        ("lib/other.js", "export function foo() { return 2 }\n"),
        (
            "packages/tonly/package.json",
            "{\"name\":\"tonly\",\"types\":\"types.d.ts\"}\n",
        ),
        (
            "packages/tonly/index.js",
            "export function foo() { return 1 }\n",
        ),
        (
            "packages/tonly/types.d.ts",
            "export declare function foo(): number;\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("packages/tonly/index.js"));

    assert_site(&report, "app/use.js", 2, SiteLabel::Proven, "import");
}

fn module_only_package_with(index: &str) -> Repo {
    scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "app/a.cjs",
            "const { foo } = require('pkg')\nmodule.exports = function run() { return foo() }\n",
        ),
        (
            "packages/pkg/package.json",
            "{\"name\":\"pkg\",\"module\":\"esm/index.js\"}\n",
        ),
        ("packages/pkg/index.js", index),
        (
            "packages/pkg/esm/index.js",
            "export function foo() { return 1 }\n",
        ),
    ])
}

#[test]
fn a_module_exports_property_assignment_is_an_export_of_its_file() {
    let repo = module_only_package_with("module.exports.foo = function foo() { return 2 }\n");

    let index = report(&repo, "foo", Some("packages/pkg/index.js"));
    assert_site(&index, "app/a.cjs", 2, SiteLabel::Proven, "import");
    let esm = report(&repo, "foo", Some("packages/pkg/esm/index.js"));
    assert_site(
        &esm,
        "app/a.cjs",
        2,
        SiteLabel::Excluded,
        "resolves to `foo` (packages/pkg/index.js:1)",
    );
}

#[test]
fn a_property_of_an_object_named_like_exports_is_not_an_export_of_its_file() {
    let repo = module_only_package_with(
        "const reexports = {}\nreexports.foo = function foo() { return 2 }\n",
    );

    let index = report(&repo, "foo", Some("packages/pkg/index.js"));
    assert_ne!(
        site_at(&index, "app/a.cjs", 2).label,
        SiteLabel::Proven,
        "{}",
        dump(&index)
    );
    let esm = report(&repo, "foo", Some("packages/pkg/esm/index.js"));
    assert_site(
        &esm,
        "app/a.cjs",
        2,
        SiteLabel::Unresolved,
        "the import matches 2 definitions",
    );
}

const CONFIG_DIR_ENGINE_FILES: &[(&str, &str)] = &[
    ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
    (
        "packages/app/src/app.ts",
        "import { helper } from 'core'\nexport function run() { return helper() }\n",
    ),
    (
        "packages/app/src/engine/index.ts",
        "export function helper() { return 1 }\n",
    ),
    (
        "packages/app/src/other/util.ts",
        "export function helper() { return 2 }\n",
    ),
];

fn assert_engine_helper_wins(repo: &Repo) {
    let engine = report(repo, "helper", Some("packages/app/src/engine/index.ts"));
    assert_site(
        &engine,
        "packages/app/src/app.ts",
        2,
        SiteLabel::Proven,
        "import",
    );
    let other = report(repo, "helper", Some("packages/app/src/other/util.ts"));
    assert_site(
        &other,
        "packages/app/src/app.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (packages/app/src/engine/index.ts:1)",
    );
}

fn config_dir_engine_with(configs: &[(&str, &str)]) -> Repo {
    let files: Vec<(&str, &str)> = CONFIG_DIR_ENGINE_FILES
        .iter()
        .chain(configs)
        .copied()
        .collect();
    scanned_repo(&files)
}

#[test]
fn a_config_dir_template_in_a_paths_target_starts_at_the_folder_of_the_config() {
    let repo = config_dir_engine_with(&[(
        "packages/app/tsconfig.json",
        "{ \"compilerOptions\": { \"paths\": { \"core\": [\"${configDir}/src/engine/index.ts\"] } } }\n",
    )]);

    assert_engine_helper_wins(&repo);
}

#[test]
fn a_config_dir_template_in_the_base_url_of_an_extended_config_starts_at_the_folder_of_the_config_in_use()
 {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "packages/app/tsconfig.json",
            "{ \"extends\": \"../../tsconfig.base.json\" }\n",
        ),
        (
            "tsconfig.base.json",
            "{ \"compilerOptions\": { \"baseUrl\": \"${configDir}/src\" } }\n",
        ),
        (
            "packages/app/src/app.ts",
            "import { helper } from 'engine'\nexport function run() { return helper() }\n",
        ),
        (
            "packages/app/src/engine/index.ts",
            "export function helper() { return 1 }\n",
        ),
        (
            "legacy/engine/index.ts",
            "export function helper() { return 2 }\n",
        ),
    ]);

    let engine = report(&repo, "helper", Some("packages/app/src/engine/index.ts"));
    assert_site(
        &engine,
        "packages/app/src/app.ts",
        2,
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_config_dir_template_in_an_extended_config_starts_at_the_folder_of_the_config_in_use() {
    let repo = config_dir_engine_with(&[
        (
            "packages/app/tsconfig.json",
            "{ \"extends\": \"../../tsconfig.base.json\" }\n",
        ),
        (
            "tsconfig.base.json",
            "{ \"compilerOptions\": { \"paths\": { \"core\": [\"${configDir}/src/engine/index.ts\"] } } }\n",
        ),
    ]);

    assert_engine_helper_wins(&repo);
}

#[test]
fn a_config_dir_template_in_a_base_url_starts_at_the_folder_of_the_config() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"root\",\"private\":true}\n"),
        (
            "tsconfig.json",
            "{ \"compilerOptions\": { \"baseUrl\": \"${configDir}/src\" } }\n",
        ),
        (
            "src/app.ts",
            "import { helper } from 'engine'\nexport function run() { return helper() }\n",
        ),
        (
            "src/engine/index.ts",
            "export function helper() { return 1 }\n",
        ),
        (
            "legacy/engine/index.ts",
            "export function helper() { return 2 }\n",
        ),
    ]);

    let engine = report(&repo, "helper", Some("src/engine/index.ts"));
    assert_site(&engine, "src/app.ts", 2, SiteLabel::Proven, "import");
    let legacy = report(&repo, "helper", Some("legacy/engine/index.ts"));
    assert_site(
        &legacy,
        "src/app.ts",
        2,
        SiteLabel::Excluded,
        "resolves to `helper` (src/engine/index.ts:1)",
    );
}

#[test]
fn any_other_template_in_a_paths_target_leaves_the_module_unknown() {
    let repo = config_dir_engine_with(&[(
        "packages/app/tsconfig.json",
        "{ \"compilerOptions\": { \"paths\": { \"core\": [\"${other}/src/engine/index.ts\"] } } }\n",
    )]);

    for target in [
        "packages/app/src/engine/index.ts",
        "packages/app/src/other/util.ts",
    ] {
        let report = report(&repo, "helper", Some(target));
        assert_ne!(
            site_at(&report, "packages/app/src/app.ts", 2).label,
            SiteLabel::Excluded,
            "{target}\n{}",
            dump(&report)
        );
    }
}

#[test]
fn a_typed_arrow_in_either_branch_of_a_ternary_binds_its_parameters() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function f() { return 1 }\n"),
        (
            "use.ts",
            "import { f } from './m';\nexport function consequent(c: boolean) {\n  return c ? (f: () => number): number => f() : 2;\n}\nexport function alternate(c: boolean) {\n  return c ? 1 : (f: () => number): number => f();\n}\nexport function plain() { return f(); }\n",
        ),
    ]);
    let report = report(&repo, "f", Some("m.ts"));

    assert_site(&report, "use.ts", 3, SiteLabel::Likely, &binding_note(3));
    assert_site(&report, "use.ts", 6, SiteLabel::Likely, &binding_note(6));
    assert_site(&report, "use.ts", 8, SiteLabel::Proven, "import");
}

fn proven_after_parentheses(body: &str) -> Repo {
    scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            &format!(
                "import {{ foo }} from './m';\nexport function use(z: any, c: boolean, p: any) {{\n{body}\n  return foo();\n}}\n"
            ),
        ),
    ])
}

fn assert_return_proven(repo: &Repo) {
    let report = report(repo, "foo", Some("m.ts"));
    let line = line_of(repo, "use.ts", "return foo()");
    assert_site(&report, "use.ts", line, SiteLabel::Proven, "import");
}

#[test]
fn an_object_given_to_a_method_named_function_does_not_bind_its_values() {
    assert_return_proven(&proven_after_parentheses(
        "  const fn = z.function({ input: foo, output: 1 });",
    ));
}

#[test]
fn a_property_named_function_times_a_group_does_not_bind_the_names_in_it() {
    assert_return_proven(&proven_after_parentheses(
        "  const n = z.function * (foo ? 1 : 2);",
    ));
}

#[test]
fn a_for_method_call_is_not_a_loop_head() {
    assert_return_proven(&proven_after_parentheses("  z.for(const foo of p);"));
}

#[test]
fn a_group_followed_by_an_arrow_past_the_return_type_bound_is_not_a_typed_arrow() {
    let far = "a".repeat(220);
    let near = "a".repeat(150);
    let rest = "a".repeat(100);
    assert_return_proven(&proven_after_parentheses(&format!(
        "  const v = c ? (foo) : {far} => 0;\n  const w = c ? (foo) : '{near} => {rest}';"
    )));
}

#[test]
fn a_group_in_a_ternary_before_a_nested_ternary_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const v = c ? (foo) : p ? 1 : (n: number) => n;",
    ));
}

#[test]
fn a_group_in_a_ternary_before_an_object_of_arrows_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const v = c ? (foo) : { run: (n: number) => n };",
    ));
}

#[test]
fn a_group_after_a_logical_operator_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const a = c ? p && (foo) : (n: number) => n;\n  const b = c ? p || (foo) : (n: number) => n;\n  const d = c ? p ?? (foo) : (n: number) => n;",
    ));
}

#[test]
fn a_group_in_a_ternary_inside_a_call_is_not_a_typed_arrow_of_the_next_call() {
    assert_return_proven(&proven_after_parentheses(
        "  wrap(c ? (foo) : 1).then((r: number) => r);",
    ));
}

#[test]
fn a_group_in_a_ternary_before_an_arrow_on_the_next_line_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const a = c ? (foo || 1) : null\n  const f = (n: number) => n",
    ));
}

#[test]
fn a_group_in_a_ternary_before_a_statement_end_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const v = c ? (foo) : 1; const f = (n: number) => n;",
    ));
}

#[test]
fn a_method_named_like_a_keyword_does_not_make_a_group_of_its_arguments() {
    assert_return_proven(&proven_after_parentheses(
        "  const a = c ? z.return(foo) : (n: number): number => n;\n  const b = c ? z.delete(foo) : (n: number): number => n;",
    ));
}

#[test]
fn a_group_in_a_ternary_at_the_end_of_an_object_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  wrap({ a: c ? (foo) : 1 }, { run: (n: number) => n });",
    ));
}

#[test]
fn a_group_after_an_operator_keyword_in_a_ternary_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const a = c ? await (foo) : (n: number) => n;\n  const b = c ? typeof (foo) : (n: number) => n;\n  const d = c ? void (foo) : (n: number) => n;\n  const e = c ? delete (foo) : (n: number) => n;\n  const g = c ? 'k' in (foo) : (n: number) => n;\n  const h = c ? of(foo) : (n: number) => n;\n  switch (p) { case (foo): z.cb = (n: number) => n; }",
    ));
}

#[test]
fn a_group_in_a_ternary_before_a_later_argument_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  wrap(c ? (foo) : { a: 1 }, (n: number) => n);\n  const o = { a: c ? (foo) : { b: 1 }, f: (n: number) => n };",
    ));
}

#[test]
fn a_group_in_a_ternary_before_a_closing_bracket_or_a_comparison_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  p[c ? (foo) : 1] = [(n: number) => n];\n  const v = c ? (foo) : p > 1 && [(n: number) => n];",
    ));
}

#[test]
fn a_group_in_a_line_comment_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  // keep foo (foo):\n  const f = (n: number) => n;\n  const u = 'http://x', v = c ? of(foo) : (n: number) => n;",
    ));
}

#[test]
fn a_call_on_an_index_result_in_a_ternary_is_not_a_typed_arrow() {
    assert_return_proven(&proven_after_parentheses(
        "  const v = c ? p[0](foo) : (n: number) => n;",
    ));
}

#[test]
fn a_typed_arrow_binds_its_parameters_whatever_shape_its_return_type_has() {
    let calls = [
        "items.map((foo: () => number): Promise<\n    Array<number>\n  > => Promise.resolve([foo()]))",
        "items.map((foo: () => number):\n    | number\n    | string => foo())",
        "items.map((foo: () => number): number |\n    string => foo())",
        "items.map((foo: () => number): Map<\n    string,\n    number\n  > => new Map([['k', foo()]]))",
        "items.map((foo: () => number): Map<string, number> => new Map([['k', foo()]]))",
        "items.map((foo: () => number): [number, string?] => [foo(), ''])",
        "items.map((foo: () => number): Array<() => number> => [() => foo()])",
        "items.map((foo: () => number): 'a?' | \"(\" | `b,${number}` | '?\"' => (foo() ? 'a?' : '('))",
        "items.map((foo: () => number): 'it\\'s' | 'x' => (foo() ? 'x' : 'x'))",
        "items.map((foo: () => number): {\n    // the user's id\n    v: number;\n  } => ({ v: foo() }))",
        "items.map((foo: () => number): {\n    /** x < y */\n    v: number;\n  } => ({ v: foo() }))",
        "items.map((foo: () => number): number /* don't, ever */ => foo())",
        "items.map((foo: () => number): \r\n    number => foo())",
        "items.map((foo: () => number): Base &\n    Extra => foo() as any)",
        "items.map((foo: () => number): Base\n    & Extra => foo() as any)",
        "items.length\n    ? 1\n    : // see http://x.org/a#b\n      // and the note\n      (foo: () => number): number => foo()",
        "items.map((foo: () => number):\n    number => foo())",
        "items.map(async (foo: () => number): Promise<number> => foo())",
    ];
    let mut source = String::from("import { foo } from './m';\n");
    let mut expected = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        let first = source.lines().count() + 2;
        let line_with =
            |needle| first + call.lines().position(|line| line.contains(needle)).unwrap();
        expected.push((line_with("foo()"), line_with("(foo")));
        source.push_str(&format!(
            "export function f{index}(items: Array<() => number>) {{\n  return {call};\n}}\n"
        ));
    }
    source.push_str("export function plain() { return foo(); }\n");
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        ("use.ts", &source),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for (call, binding) in expected {
        assert_site(
            &report,
            "use.ts",
            call,
            SiteLabel::Likely,
            &binding_note(binding),
        );
    }
    assert_site(
        &report,
        "use.ts",
        source.lines().count(),
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_typed_arrow_after_a_statement_keyword_binds_its_parameters() {
    let statements = [
        "return (foo: () => number): number => foo();",
        "if (flag) return (foo: () => number): number => foo();",
        "yield (foo: () => number): number => foo();",
        "throw (foo: () => number): number => foo();",
        "if (flag) { return 1; } else (foo: () => number): number => foo();",
        "do (foo: () => number): number => foo(); while (flag);",
    ];
    let mut source = String::from("import { foo } from './m';\n");
    for (index, statement) in statements.iter().enumerate() {
        source.push_str(&format!(
            "export async function* f{index}(flag: boolean) {{\n  {statement}\n}}\n"
        ));
    }
    source.push_str("export function plain() { return foo(); }\n");
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        ("use.ts", &source),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for index in 0..statements.len() {
        let line = 3 + 3 * index;
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Likely,
            &binding_note(line),
        );
    }
    assert_site(
        &report,
        "use.ts",
        2 + 3 * statements.len(),
        SiteLabel::Proven,
        "import",
    );
}

#[test]
fn a_typed_arrow_with_an_object_return_type_binds_its_parameters() {
    let long_type = "v: number; ".repeat(14);
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            &format!(
                "import {{ foo }} from './m';\nexport function a(items: Array<() => number>) {{\n  return items.map((foo: () => number): {{ v: number }} => ({{ v: foo() }}));\n}}\nexport function b(items: Array<() => number>) {{\n  return items.map((foo: () => number): {{ v?: number; w: string }} => ({{ v: foo(), w: '' }}));\n}}\nexport function c(items: Array<() => number>) {{\n  return items.map((foo: () => number): {{\n    v: number;\n  }} => ({{ v: foo() }}));\n}}\nexport function d(items: Array<() => number>) {{\n  return items.map((foo: () => number): {{ {long_type}}} => ({{ v: foo() }}));\n}}\nexport function plain() {{ return foo(); }}\n"
            ),
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for (call, binding) in [(3, 3), (6, 6), (11, 9), (14, 14)] {
        assert_site(
            &report,
            "use.ts",
            call,
            SiteLabel::Likely,
            &binding_note(binding),
        );
    }
    assert_site(&report, "use.ts", 16, SiteLabel::Proven, "import");
}

#[test]
fn a_java_read_of_a_local_variable_is_still_hidden_by_it() {
    let repo = scanned_repo(&[(
        "q/Trade.java",
        "package q;\n\npublic class Trade {\n    private int price;\n\n    int total() {\n        int price = 2;\n        return price;\n    }\n}\n",
    )]);
    let report = report(&repo, "price", Some("q/Trade.java"));

    assert_site(
        &report,
        "q/Trade.java",
        8,
        SiteLabel::Excluded,
        "resolves to `price` (q/Trade.java:7)",
    );
}

#[test]
fn a_typescript_type_use_is_not_shadowed_by_the_value_of_the_same_name() {
    let path = "src/checks.ts";
    let repo = scanned_repo(&[(
        path,
        "export interface Check {\n  kind: string;\n}\n\nexport const Check = (kind: string): Check => ({ kind });\n\nexport interface Other {\n  size: number;\n}\n\nexport type Checks =\n  | Other\n  | Check;\n",
    )]);
    let report = first_definition_report(&repo, "Check", path);

    assert_not_excluded(&report, path, line_of(&repo, path, "  | Check;"));
}

const RUST_MODULE_AND_LOCAL: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    (
        "src/lib.rs",
        "mod git;\n\npub struct Pending {\n    git: u8,\n}\n\npub fn run(p: Pending) -> u8 {\n    let Pending { git } = p;\n    double(git)\n}\n\nfn double(n: u8) -> u8 {\n    n * 2\n}\n",
    ),
    ("src/git.rs", "pub fn diff() {}\n"),
    (
        "tests/check.rs",
        "fn git() -> u8 {\n    1\n}\n\n#[test]\nfn uses_git() {\n    assert_eq!(git(), 1);\n}\n",
    ),
];

#[test]
fn a_rust_read_of_a_value_never_resolves_to_a_module_of_its_name() {
    let repo = scanned_repo(RUST_MODULE_AND_LOCAL);
    let report = first_definition_report(&repo, "git", "tests/check.rs");

    let site = site_at(
        &report,
        "src/lib.rs",
        line_of(&repo, "src/lib.rs", "double(git)"),
    );
    assert!(!site.basis.contains("src/lib.rs:1)"), "{}", dump(&report));
}

#[test]
fn a_rust_module_is_not_the_target_of_a_read_of_a_value() {
    let repo = scanned_repo(RUST_MODULE_AND_LOCAL);
    let report = first_definition_report(&repo, "git", "src/lib.rs");

    assert_site(
        &report,
        "src/lib.rs",
        line_of(&repo, "src/lib.rs", "double(git)"),
        SiteLabel::Excluded,
        "`git` is a module, which a read of a value never names",
    );
}

#[test]
fn a_java_call_that_a_static_import_ties_to_another_class_is_excluded() {
    let repo = scanned_repo(&[
        (
            "p/TypeAdapters.java",
            "package p;\n\npublic final class TypeAdapters {\n    static int toIntExact(long v) {\n        return (int) v;\n    }\n}\n",
        ),
        (
            "p/Time.java",
            "package p;\n\nimport static java.lang.Math.toIntExact;\n\npublic final class Time {\n    int of(long v) {\n        return toIntExact(v);\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "toIntExact", Some("p/TypeAdapters.java"));

    assert_site(
        &report,
        "p/Time.java",
        7,
        SiteLabel::Excluded,
        "`toIntExact` is imported from `java.lang.Math`",
    );
}

#[test]
fn a_java_call_is_not_shadowed_by_a_field_of_its_name() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    protected int price() {\n        return 1;\n    }\n}\n",
        ),
        (
            "q/Trade.java",
            "package q;\n\nimport p.A;\n\npublic class Trade extends A {\n    private int price;\n\n    int total() {\n        return price();\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "price", Some("p/A.java"));

    assert_ne!(
        site_at(&report, "q/Trade.java", 9).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_java_call_is_not_shadowed_by_a_local_variable_or_parameter_of_its_name() {
    let repo = scanned_repo(&[
        (
            "p/A.java",
            "package p;\n\npublic class A {\n    protected int price() {\n        return 1;\n    }\n}\n",
        ),
        (
            "q/Trade.java",
            "package q;\n\nimport p.A;\n\npublic class Trade extends A {\n    int local() {\n        int price = 2;\n        return price();\n    }\n\n    int param(int price) {\n        return price();\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "price", Some("p/A.java"));

    for line in [8, 12] {
        assert_ne!(
            site_at(&report, "q/Trade.java", line).label,
            SiteLabel::Excluded,
            "{line}\n{}",
            dump(&report)
        );
    }
}

const PY_OVERRIDES: &[(&str, &str)] = &[
    (
        "pkg/base.py",
        "class Base:\n    def render(self):\n        return 1\n\n\nclass Mid(Base):\n    def render(self):\n        return 2\n",
    ),
    (
        "pkg/leaf.py",
        "from pkg.base import Mid\n\n\nclass Leaf(Mid):\n    def render(self):\n        return 3\n\n    def other(self):\n        return 4\n\n\ndef free():\n    return 5\n",
    ),
];

fn listed(rows: &[OverrideRow]) -> Vec<String> {
    rows.iter()
        .map(|row| format!("{}.{} {}:{}", row.owner, row.name, row.path, row.line))
        .collect()
}

fn report_at(repo: &Repo, name: &str, path: &str, line: usize) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND start_line = ?3
             AND kind NOT IN ('import', 'export')",
            rusqlite::params![name, path, line as i64],
            |row| row.get(0),
        )
        .unwrap();
    let target = get_symbol_by_id(&conn, &id).unwrap().unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

#[test]
fn a_python_method_lists_the_overrides_in_every_subclass_and_the_methods_it_overrides() {
    let repo = scanned_repo(PY_OVERRIDES);

    let base = report(&repo, "Base.render", None);
    assert_eq!(
        listed(&base.overrides),
        ["Mid.render pkg/base.py:7", "Leaf.render pkg/leaf.py:5"],
        "{}",
        dump(&base)
    );
    assert!(base.overridden.is_empty());

    let leaf = report(&repo, "Leaf.render", None);
    assert_eq!(
        listed(&leaf.overridden),
        ["Mid.render pkg/base.py:7", "Base.render pkg/base.py:2"]
    );
    assert!(leaf.overrides.is_empty());

    let mid = report(&repo, "Mid.render", None);
    assert_eq!(listed(&mid.overrides), ["Leaf.render pkg/leaf.py:5"]);
    assert_eq!(listed(&mid.overridden), ["Base.render pkg/base.py:2"]);
}

#[test]
fn override_rows_carry_the_symbol_id_of_the_method() {
    let repo = scanned_repo(PY_OVERRIDES);

    let base = report(&repo, "Base.render", None);

    let conn = open_read_only(&repo.db).unwrap();
    let symbol = get_symbol_by_id(&conn, &base.overrides[0].symbol_id)
        .unwrap()
        .unwrap();
    assert_eq!((symbol.name.as_str(), symbol.start_line), ("render", 7));
}

#[test]
fn a_method_nobody_overrides_and_a_free_function_print_neither_section() {
    let repo = scanned_repo(PY_OVERRIDES);

    for name in ["Leaf.other", "free"] {
        let report = report(&repo, name, None);
        assert!(report.overrides.is_empty() && report.overridden.is_empty());
        let text = format_reference_report(&report, 20);
        assert!(!text.contains("Overrides"), "{text}");
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.get("overrides").is_none() && json.get("overridden").is_none());
    }
}

#[test]
fn the_override_sections_print_after_the_sites_and_before_the_text_only_lines() {
    let repo = scanned_repo(&[
        PY_OVERRIDES[0],
        PY_OVERRIDES[1],
        (
            "pkg/use.py",
            "from pkg.base import Base\n\n\ndef go(b: Base):\n    return b.render()\n",
        ),
        ("notes.md", "render is documented here\n"),
    ]);

    let base = format_reference_report(&report(&repo, "Base.render", None), 20);
    assert!(
        base.contains(
            "- `go` [pkg/use.py:5] (kind: calls; receiver type; 0 args)\n\
             Overrides (2): calls through `Base.render` can reach these\n\
             - `Mid.render` (pkg/base.py:7)\n\
             - `Leaf.render` (pkg/leaf.py:5)\n\
             Text only (1): "
        ),
        "{base}"
    );

    let mid = format_reference_report(&report(&repo, "Mid.render", None), 20);
    assert!(
        mid.contains(
            "Overrides (1): calls through `Mid.render` can reach these\n\
             - `Leaf.render` (pkg/leaf.py:5)\n\
             Overrides of (1): calls to these can reach `Mid.render`\n\
             - `Base.render` (pkg/base.py:2)\n\
             Text only (1): "
        ),
        "{mid}"
    );
}

#[test]
fn override_rows_fold_after_the_limit_with_the_full_count() {
    let mut files = vec![(
        "pkg/base.py".to_string(),
        "class Base:\n    def run(self):\n        return 0\n".to_string(),
    )];
    for n in 1..=3 {
        files.push((
            format!("pkg/sub{n}.py"),
            format!("from pkg.base import Base\n\n\nclass Sub{n}(Base):\n    def run(self):\n        return {n}\n"),
        ));
    }
    let repo = scanned_owned(&files);

    let report = report(&repo, "Base.run", None);
    let text = format_reference_report(&report, 2);

    assert_eq!(report.overrides.len(), 3);
    assert!(
        text.contains(
            "Overrides (3): calls through `Base.run` can reach these\n\
             - `Sub1.run` (pkg/sub1.py:5)\n\
             - `Sub2.run` (pkg/sub2.py:5)\n\
             … 1 more method in 1 file: pkg/sub3.py (1)\n"
        ),
        "{text}"
    );
}

#[test]
fn a_subclass_that_a_nearer_class_of_the_same_name_shadows_is_not_counted() {
    let repo = scanned_repo(&[
        (
            "a/base.py",
            "class Base:\n    def run(self):\n        return 1\n",
        ),
        (
            "b/base.py",
            "class Base:\n    def run(self):\n        return 2\n",
        ),
        (
            "b/sub.py",
            "from base import Base\n\n\nclass Sub(Base):\n    def run(self):\n        return 3\n",
        ),
    ]);

    let far = first_definition_report(&repo, "run", "a/base.py");
    let near = first_definition_report(&repo, "run", "b/base.py");

    assert!(far.overrides.is_empty(), "{:?}", far.overrides);
    assert_eq!(listed(&near.overrides), ["Sub.run b/sub.py:5"]);
}

#[test]
fn a_base_class_that_a_nearer_class_of_the_same_name_shadows_is_not_listed() {
    let repo = scanned_repo(&[
        (
            "a/base.py",
            "class Base:\n    def run(self):\n        return 1\n",
        ),
        (
            "b/base.py",
            "class Base:\n    def run(self):\n        return 2\n",
        ),
        (
            "b/sub.py",
            "from base import Base\n\n\nclass Sub(Base):\n    def run(self):\n        return 3\n",
        ),
    ]);

    let report = first_definition_report(&repo, "run", "b/sub.py");

    assert_eq!(listed(&report.overridden), ["Base.run b/base.py:2"]);
}

#[test]
fn typescript_interface_and_abstract_class_methods_list_their_implementations() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  area(): number;\n}\nexport abstract class Poly implements Shape {\n  abstract area(): number;\n}\nexport class Square extends Poly {\n  area(): number { return 1; }\n}\n",
    )]);

    let shape = report(&repo, "Shape.area", None);
    assert_eq!(
        listed(&shape.overrides),
        ["Poly.area src/shapes.ts:5", "Square.area src/shapes.ts:8"]
    );

    let square = report(&repo, "Square.area", None);
    assert_eq!(
        listed(&square.overridden),
        ["Poly.area src/shapes.ts:5", "Shape.area src/shapes.ts:2"]
    );
}

#[test]
fn a_typescript_interface_method_in_object_shorthand_stays_an_unresolved_candidate() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nfunction make(tx: (value: string) => string): Shape {\n  return { tx };\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.ts", "return { tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.proven, 0, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 1, "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let binding_line = line_of(&repo, "src/shapes.ts", "function make(tx");
    let local = report_at(&repo, "tx", "src/shapes.ts", binding_line);
    let local_site = local
        .sites
        .iter()
        .find(|site| site.site.start_line == Some(site_line))
        .unwrap();

    assert_ne!(local_site.label, SiteLabel::Excluded, "{}", dump(&local));
}

#[test]
fn a_typescript_interface_method_candidate_adds_no_test_impact_edge() {
    let repo = scanned_repo(&[
        (
            "package.json",
            "{\"devDependencies\":{\"vitest\":\"^1\"}}\n",
        ),
        (
            "src/shapes.ts",
            "export interface Shape {\n  tx(value: string): string;\n}\n",
        ),
        (
            "src/shapes.test.ts",
            "test(\"shorthand\", (tx: (value: string) => string) => {\n  return { tx };\n});\n",
        ),
    ]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.test.ts", "return { tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.proven, 0, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 1, "{}", dump(&report));
    assert!(report.tests.is_empty(), "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.test.ts",
        site_line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_typescript_interface_method_in_a_constructor_argument_stays_unresolved_on_crlf() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\r\n  tx(value: string): string;\r\n}\r\nclass Box<T> { constructor(value: T) {} }\r\nfunction make(tx: (value: string) => string): Shape {\r\n  return new Box({ tx });\r\n}\r\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.ts", "return new Box({ tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.proven, 0, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 1, "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_typescript_interface_method_in_a_multiline_overwrite_constructor_stays_unresolved() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nfunction make(tx: (value: string) => string): Shape {\n  return new checks.$ZodCheckOverwrite({\n    check: \"overwrite\",\n    tx,\n  });\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.ts", "    tx,");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.proven, 0, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 1, "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_typescript_interface_method_in_a_nested_multiline_call_argument_stays_excluded() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nfunction make(tx: (value: string) => string) {\n  return new checks.$ZodCheckOverwrite({\n    check: \"overwrite\",\n    callback: transform(\"value\",\n      tx,\n    ),\n  });\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let binding_line = line_of(&repo, "src/shapes.ts", "function make(tx");
    let site_line = line_of(&repo, "src/shapes.ts", "      tx,");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Excluded,
        &format!("resolves to `tx` (src/shapes.ts:{binding_line})"),
    );
}

#[test]
fn a_typescript_interface_method_in_an_unterminated_final_source_line_stays_unresolved() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nfunction make(tx: (value: string) => string): Shape {\n  return { tx }; }",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.ts", "return { tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.proven, 0, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 1, "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn a_typescript_interface_method_local_read_still_excludes_the_target() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nfunction make(tx: (value: string) => string): string {\n  return tx(\"value\");\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let binding_line = line_of(&repo, "src/shapes.ts", "function make(tx");
    let site_line = line_of(&repo, "src/shapes.ts", "return tx(");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Excluded,
        &format!("resolves to `tx` (src/shapes.ts:{binding_line})"),
    );
}

#[test]
fn a_typescript_interface_method_property_value_destructuring_and_block_reads_stay_excluded() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\ntype Fn = (value: string) => string;\nfunction make(tx: Fn, source: { tx: Fn }) {\n  const { tx: renamed } = source;\n  if (source) { tx; }\n  return { value: tx };\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let destructuring_line = line_of(&repo, "src/shapes.ts", "const { tx: renamed }");
    let block_line = line_of(&repo, "src/shapes.ts", "if (source) { tx; }");
    let property_value_line = line_of(&repo, "src/shapes.ts", "return { value: tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_eq!(report.counts.unresolved, 0, "{}", dump(&report));
    assert_site(
        &report,
        "src/shapes.ts",
        block_line,
        SiteLabel::Excluded,
        "resolves to `tx` (src/shapes.ts:5)",
    );
    assert_site(
        &report,
        "src/shapes.ts",
        property_value_line,
        SiteLabel::Excluded,
        "resolves to `tx` (src/shapes.ts:5)",
    );
    if report
        .sites
        .iter()
        .any(|site| site.site.start_line == Some(destructuring_line))
    {
        assert_site(
            &report,
            "src/shapes.ts",
            destructuring_line,
            SiteLabel::Excluded,
            "resolves to `tx` (src/shapes.ts:5)",
        );
    }
}

#[test]
fn a_typescript_concrete_class_method_in_object_shorthand_still_excludes_the_target() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export class Shape {\n  tx(value: string): string { return value; }\n}\nfunction make(tx: (value: string) => string): Shape {\n  return { tx };\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let binding_line = line_of(&repo, "src/shapes.ts", "function make(tx");
    let site_line = line_of(&repo, "src/shapes.ts", "return { tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Excluded,
        &format!("resolves to `tx` (src/shapes.ts:{binding_line})"),
    );
}

#[test]
fn a_typescript_function_target_in_object_shorthand_still_excludes_a_same_named_parameter() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export function tx(value: string): string { return value; }\nfunction make(tx: (value: string) => string) {\n  return { tx };\n}\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "function tx(");
    let binding_line = line_of(&repo, "src/shapes.ts", "function make(tx");
    let site_line = line_of(&repo, "src/shapes.ts", "return { tx }");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Excluded,
        &format!("resolves to `tx` (src/shapes.ts:{binding_line})"),
    );
}

#[test]
fn a_typescript_interface_method_does_not_reach_a_different_typed_owner() {
    let repo = scanned_repo(&[(
        "src/shapes.ts",
        "export interface Shape {\n  tx(value: string): string;\n}\nclass Other { tx(value: string): string { return value; } }\nfunction use(other: Other) { return other.tx(\"value\"); }\n",
    )]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/shapes.ts", "other.tx(");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/shapes.ts",
        site_line,
        SiteLabel::Excluded,
        "resolves to `Other.tx` (src/shapes.ts:4)",
    );
}

#[test]
fn a_typescript_interface_method_import_and_export_keep_the_external_binding_exclusion() {
    let repo = scanned_repo(&[
        (
            "src/shapes.ts",
            "export interface Shape {\n  tx(value: string): string;\n}\n",
        ),
        (
            "src/use.ts",
            "import { tx } from 'external';\nexport { tx };\nexport function use() { return tx(\"value\"); }\n",
        ),
    ]);
    let target_line = line_of(&repo, "src/shapes.ts", "tx(value: string)");
    let site_line = line_of(&repo, "src/use.ts", "return tx(");
    let report = report_at(&repo, "tx", "src/shapes.ts", target_line);

    assert_site(
        &report,
        "src/use.ts",
        site_line,
        SiteLabel::Excluded,
        "receiver `tx` is imported from `external`",
    );
}

#[test]
fn java_interface_methods_list_the_methods_of_implementing_classes() {
    let repo = scanned_repo(&[
        (
            "Animal.java",
            "public interface Animal {\n    String speak();\n}\n",
        ),
        (
            "Dog.java",
            "public class Dog implements Animal {\n    public String speak() { return \"woof\"; }\n}\n",
        ),
    ]);

    let animal = report(&repo, "Animal.speak", None);
    assert_eq!(listed(&animal.overrides), ["Dog.speak Dog.java:2"]);

    let dog = report(&repo, "Dog.speak", None);
    assert_eq!(listed(&dog.overridden), ["Animal.speak Animal.java:2"]);
}

#[test]
fn csharp_virtual_methods_list_their_overrides() {
    let repo = scanned_repo(&[(
        "Shapes.cs",
        "public class Shape {\n    public virtual double Area() { return 0; }\n}\npublic class Circle : Shape {\n    public override double Area() { return 1; }\n}\n",
    )]);

    let shape = report(&repo, "Shape.Area", None);
    assert_eq!(listed(&shape.overrides), ["Circle.Area Shapes.cs:5"]);

    let circle = report(&repo, "Circle.Area", None);
    assert_eq!(listed(&circle.overridden), ["Shape.Area Shapes.cs:2"]);
}

#[test]
fn a_private_csharp_method_has_no_overrides_and_overrides_nothing() {
    let repo = scanned_repo(&[(
        "Readers.cs",
        "public class JsonReader {\n    private int Pop() { return 0; }\n    public virtual void Close() { }\n}\npublic class ValidatingReader : JsonReader {\n    private string Pop() { return \"\"; }\n    private void Close() { }\n}\n",
    )]);

    let base = report(&repo, "JsonReader.Pop", None);
    assert!(base.overrides.is_empty(), "{:?}", base.overrides);

    let sub = report(&repo, "ValidatingReader.Pop", None);
    assert!(sub.overridden.is_empty(), "{:?}", sub.overridden);

    let close = report(&repo, "JsonReader.Close", None);
    assert!(close.overrides.is_empty(), "{:?}", close.overrides);
}

#[test]
fn a_rust_trait_method_lists_the_impl_methods_and_an_impl_method_lists_the_trait_method() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub trait Shape {\n    fn area(&self) -> f64;\n}\npub struct Square;\nimpl Shape for Square {\n    fn area(&self) -> f64 { 1.0 }\n}\nimpl Square {\n    pub fn side(&self) -> f64 { 1.0 }\n}\n",
    )]);

    let shape = report(&repo, "Shape.area", None);
    assert_eq!(listed(&shape.overrides), ["Square.area src/lib.rs:6"]);

    let square = report(&repo, "Square.area", None);
    assert_eq!(listed(&square.overridden), ["Shape.area src/lib.rs:2"]);

    let side = report(&repo, "Square.side", None);
    assert!(side.overrides.is_empty() && side.overridden.is_empty());
}

const RS_INHERENT_AND_TRAIT_AREA: &str = r#"pub trait Shape {
    fn area(&self) -> f64;
}

pub struct Circle {
    pub r: f64,
}

pub struct Square {
    pub s: f64,
}

impl Circle {
    pub fn area(&self) -> f64 {
        self.r * 3.0
    }
}

impl Shape for Circle {
    fn area(&self) -> f64 {
        self.r * 3.14
    }
}

impl Shape for Square {
    fn area(&self) -> f64 {
        self.s * self.s
    }
}

impl Square {
    pub fn area(&self) -> f64 {
        self.s
    }
}
"#;

#[test]
fn a_rust_inherent_method_is_not_an_override_of_a_trait_method_of_the_same_name() {
    let repo = scanned_repo(&[("src/lib.rs", RS_INHERENT_AND_TRAIT_AREA)]);

    let shape = report(&repo, "Shape.area", None);

    assert_eq!(
        listed(&shape.overrides),
        ["Circle.area src/lib.rs:20", "Square.area src/lib.rs:26"],
        "{}",
        dump(&shape)
    );
}

#[test]
fn a_rust_inherent_method_overrides_nothing_and_a_trait_impl_method_lists_its_trait() {
    let repo = scanned_repo(&[("src/lib.rs", RS_INHERENT_AND_TRAIT_AREA)]);

    for line in [14, 32] {
        let inherent = report_at(&repo, "area", "src/lib.rs", line);
        assert!(
            inherent.overrides.is_empty() && inherent.overridden.is_empty(),
            "line {line}: {:?}",
            inherent.overridden
        );
    }
    for line in [20, 26] {
        let implemented = report_at(&repo, "area", "src/lib.rs", line);
        assert_eq!(
            listed(&implemented.overridden),
            ["Shape.area src/lib.rs:2"],
            "line {line}"
        );
    }
}

const RS_TWO_TRAITS: &str = r#"pub trait A {
    fn go(&self) -> i32;
}

pub trait B {
    fn go(&self) -> i32;
}

pub struct X;

impl A for X {
    fn go(&self) -> i32 {
        1
    }
}

impl<T> B for Wrapper<T>
where
    T: Clone,
{
    fn go(&self) -> i32 {
        3
    }
}

pub struct Wrapper<T>(T);

impl B for X {
    fn go(&self) -> i32 {
        2
    }
}
"#;

#[test]
fn a_rust_method_belongs_to_the_trait_of_its_own_impl_block_only() {
    let repo = scanned_repo(&[("src/lib.rs", RS_TWO_TRAITS)]);

    let a = report(&repo, "A.go", None);
    let b = report(&repo, "B.go", None);

    assert_eq!(listed(&a.overrides), ["X.go src/lib.rs:12"], "{}", dump(&a));
    assert_eq!(
        listed(&b.overrides),
        ["Wrapper.go src/lib.rs:21", "X.go src/lib.rs:29"],
        "{}",
        dump(&b)
    );
    for (line, expected) in [
        (12, "A.go src/lib.rs:2"),
        (21, "B.go src/lib.rs:6"),
        (29, "B.go src/lib.rs:6"),
    ] {
        let implemented = report_at(&repo, "go", "src/lib.rs", line);
        assert_eq!(listed(&implemented.overridden), [expected], "line {line}");
    }
}

#[test]
fn a_rust_subtrait_declaration_and_the_impl_of_a_subtrait_are_not_overrides() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub trait Base {\n    fn go(&self) -> i32;\n}\n\npub trait Sub: Base {\n    fn go(&self) -> i32;\n}\n\npub struct X;\n\nimpl Base for X {\n    fn go(&self) -> i32 {\n        1\n    }\n}\n\nimpl Sub for X {\n    fn go(&self) -> i32 {\n        2\n    }\n}\n",
    )]);

    let base = first_definition_report(&repo, "go", "src/lib.rs");

    assert_eq!(
        listed(&base.overrides),
        ["X.go src/lib.rs:12"],
        "{}",
        dump(&base)
    );
}

const RS_QUALIFIED_TRAIT: &[(&str, &str)] = &[
    ("src/lib.rs", "pub mod ovr;\n"),
    (
        "src/ovr.rs",
        "pub mod a;\n\npub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct Circle;\npub struct Tri;\n\nimpl Draw for Circle {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n\nimpl crate::ovr::a::Draw for Tri {\n    fn draw(&self) -> i32 {\n        8\n    }\n}\n",
    ),
    (
        "src/ovr/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
];

#[test]
fn a_rust_impl_with_a_qualified_trait_path_overrides_only_the_trait_in_that_file() {
    let repo = scanned_repo(RS_QUALIFIED_TRAIT);

    let near = report(&repo, "Draw.draw", Some("src/ovr.rs"));
    let named = report(&repo, "Draw.draw", Some("src/ovr/a.rs"));

    assert_eq!(
        listed(&near.overrides),
        ["Circle.draw src/ovr.rs:11"],
        "{}",
        dump(&near)
    );
    assert_eq!(
        listed(&named.overrides),
        ["Tri.draw src/ovr.rs:17"],
        "{}",
        dump(&named)
    );
}

#[test]
fn a_rust_impl_method_with_a_qualified_trait_path_lists_the_trait_in_that_file() {
    let repo = scanned_repo(RS_QUALIFIED_TRAIT);

    let qualified = report_at(&repo, "draw", "src/ovr.rs", 17);
    let plain = report_at(&repo, "draw", "src/ovr.rs", 11);

    assert_eq!(
        listed(&qualified.overridden),
        ["Draw.draw src/ovr/a.rs:2"],
        "{}",
        dump(&qualified)
    );
    assert_eq!(
        listed(&plain.overridden),
        ["Draw.draw src/ovr.rs:4"],
        "{}",
        dump(&plain)
    );
}

#[test]
fn a_rust_trait_path_through_a_mod_rs_folder_names_that_file() {
    let repo = scanned_repo(&[
        ("src/lib.rs", "pub mod shapes;\npub mod other;\n"),
        (
            "src/shapes/mod.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/other.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct Tri;\n\nimpl crate::shapes::Draw for Tri {\n    fn draw(&self) -> i32 {\n        8\n    }\n}\n",
        ),
    ]);

    let shapes = report(&repo, "Draw.draw", Some("src/shapes/mod.rs"));
    let other = report(&repo, "Draw.draw", Some("src/other.rs"));
    let tri = report_at(&repo, "draw", "src/other.rs", 8);

    assert_eq!(
        listed(&shapes.overrides),
        ["Tri.draw src/other.rs:8"],
        "{}",
        dump(&shapes)
    );
    assert!(other.overrides.is_empty(), "{:?}", other.overrides);
    assert_eq!(
        listed(&tri.overridden),
        ["Draw.draw src/shapes/mod.rs:2"],
        "{}",
        dump(&tri)
    );
}

#[test]
fn a_rust_trait_path_that_starts_with_a_crate_name_names_that_package() {
    let repo = scanned_repo(&[
        (
            "pkg_a/src/lib.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct A;\n\nimpl Draw for A {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n",
        ),
        (
            "pkg_b/src/lib.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct B;\n\nimpl pkg_a::Draw for B {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
        ),
    ]);

    let in_a = report(&repo, "Draw.draw", Some("pkg_a/src/lib.rs"));
    let in_b = report(&repo, "Draw.draw", Some("pkg_b/src/lib.rs"));

    assert_eq!(
        listed(&in_a.overrides),
        ["A.draw pkg_a/src/lib.rs:8", "B.draw pkg_b/src/lib.rs:8"],
        "{}",
        dump(&in_a)
    );
    assert!(in_b.overrides.is_empty(), "{:?}", in_b.overrides);
}

#[test]
fn a_rust_type_that_implements_two_same_named_traits_by_path_has_one_method_for_each() {
    let repo = scanned_repo(&[
        ("src/lib.rs", "pub mod a;\npub mod b;\npub mod x;\n"),
        (
            "src/a.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/b.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/x.rs",
            "pub struct X;\n\nimpl crate::a::Draw for X {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n\nimpl crate::b::Draw for X {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
        ),
    ]);

    let in_a = report(&repo, "Draw.draw", Some("src/a.rs"));
    let in_b = report(&repo, "Draw.draw", Some("src/b.rs"));
    let first = report_at(&repo, "draw", "src/x.rs", 4);
    let second = report_at(&repo, "draw", "src/x.rs", 10);

    assert_eq!(
        listed(&in_a.overrides),
        ["X.draw src/x.rs:4"],
        "{}",
        dump(&in_a)
    );
    assert_eq!(
        listed(&in_b.overrides),
        ["X.draw src/x.rs:10"],
        "{}",
        dump(&in_b)
    );
    assert_eq!(
        listed(&first.overridden),
        ["Draw.draw src/a.rs:2"],
        "{}",
        dump(&first)
    );
    assert_eq!(
        listed(&second.overridden),
        ["Draw.draw src/b.rs:2"],
        "{}",
        dump(&second)
    );
}

const RS_INLINE_MOD_TRAIT: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod shapes {\n    pub trait Draw {\n        fn draw(&self) -> i32;\n    }\n}\n\npub mod impls;\n\npub struct C;\npub struct D;\n\nimpl shapes::Draw for C {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n\nimpl self::shapes::Draw for D {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
    ),
    (
        "src/impls.rs",
        "use crate::shapes;\n\npub struct E;\npub struct F;\n\nimpl shapes::Draw for E {\n    fn draw(&self) -> i32 {\n        3\n    }\n}\n\nimpl crate::shapes::Draw for F {\n    fn draw(&self) -> i32 {\n        4\n    }\n}\n",
    ),
];

#[test]
fn a_rust_trait_path_through_an_inline_mod_overrides_the_trait_in_that_mod() {
    let repo = scanned_repo(RS_INLINE_MOD_TRAIT);

    let draw = report(&repo, "Draw.draw", Some("src/lib.rs"));

    assert_eq!(
        listed(&draw.overrides),
        [
            "E.draw src/impls.rs:7",
            "F.draw src/impls.rs:13",
            "C.draw src/lib.rs:13",
            "D.draw src/lib.rs:19",
        ],
        "{}",
        dump(&draw)
    );
}

#[test]
fn a_rust_impl_method_with_a_trait_path_through_an_inline_mod_lists_the_trait_in_that_mod() {
    let repo = scanned_repo(RS_INLINE_MOD_TRAIT);

    for (path, line) in [
        ("src/lib.rs", 13),
        ("src/lib.rs", 19),
        ("src/impls.rs", 7),
        ("src/impls.rs", 13),
    ] {
        let method = report_at(&repo, "draw", path, line);
        assert_eq!(
            listed(&method.overridden),
            ["Draw.draw src/lib.rs:3"],
            "{path}:{line}\n{}",
            dump(&method)
        );
    }
}

const RS_SUPER_IN_INLINE_MOD: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod a;\n\npub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub mod inner {\n    pub struct G;\n\n    impl super::Draw for G {\n        fn draw(&self) -> i32 {\n            1\n        }\n    }\n}\n",
    ),
    (
        "src/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
];

#[test]
fn a_rust_super_path_in_an_inline_mod_overrides_the_trait_of_the_parent_module() {
    let repo = scanned_repo(RS_SUPER_IN_INLINE_MOD);

    let parent = report(&repo, "Draw.draw", Some("src/lib.rs"));
    let sibling = report(&repo, "Draw.draw", Some("src/a.rs"));

    assert_eq!(
        listed(&parent.overrides),
        ["G.draw src/lib.rs:11"],
        "{}",
        dump(&parent)
    );
    assert!(sibling.overrides.is_empty(), "{:?}", sibling.overrides);
}

#[test]
fn a_rust_impl_method_with_a_super_path_in_an_inline_mod_lists_the_trait_of_the_parent_module() {
    let repo = scanned_repo(RS_SUPER_IN_INLINE_MOD);

    let method = report_at(&repo, "draw", "src/lib.rs", 11);

    assert_eq!(
        listed(&method.overridden),
        ["Draw.draw src/lib.rs:4"],
        "{}",
        dump(&method)
    );
}

const RS_EXACT_MODULE_PATH: &[(&str, &str)] = &[
    ("src/lib.rs", "pub mod ovr;\n"),
    (
        "src/ovr.rs",
        "pub mod a;\n\npub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct Q;\n\nimpl crate::ovr::Draw for Q {\n    fn draw(&self) -> i32 {\n        11\n    }\n}\n",
    ),
    (
        "src/ovr/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
];

#[test]
fn a_rust_trait_path_names_only_the_trait_whose_module_path_it_spells() {
    let repo = scanned_repo(RS_EXACT_MODULE_PATH);

    let in_ovr = report(&repo, "Draw.draw", Some("src/ovr.rs"));
    let in_a = report(&repo, "Draw.draw", Some("src/ovr/a.rs"));

    assert_eq!(
        listed(&in_ovr.overrides),
        ["Q.draw src/ovr.rs:10"],
        "{}",
        dump(&in_ovr)
    );
    assert!(in_a.overrides.is_empty(), "{:?}", in_a.overrides);
}

#[test]
fn a_rust_impl_method_with_a_trait_path_lists_only_the_trait_whose_module_path_it_spells() {
    let repo = scanned_repo(RS_EXACT_MODULE_PATH);

    let method = report_at(&repo, "draw", "src/ovr.rs", 10);

    assert_eq!(
        listed(&method.overridden),
        ["Draw.draw src/ovr.rs:4"],
        "{}",
        dump(&method)
    );
}

const RS_RELATIVE_TRAIT_PATH: &[(&str, &str)] = &[
    ("src/lib.rs", "pub mod ovr;\npub mod a;\n"),
    (
        "src/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    (
        "src/ovr.rs",
        "pub mod a;\n\npub struct R;\n\nimpl a::Draw for R {\n    fn draw(&self) -> i32 {\n        5\n    }\n}\n",
    ),
    (
        "src/ovr/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
];

#[test]
fn a_rust_trait_path_without_a_prefix_names_the_module_of_the_impl_before_the_crate_root() {
    let repo = scanned_repo(RS_RELATIVE_TRAIT_PATH);

    let nested = report(&repo, "Draw.draw", Some("src/ovr/a.rs"));
    let root = report(&repo, "Draw.draw", Some("src/a.rs"));

    assert_eq!(
        listed(&nested.overrides),
        ["R.draw src/ovr.rs:6"],
        "{}",
        dump(&nested)
    );
    assert!(root.overrides.is_empty(), "{:?}", root.overrides);
}

#[test]
fn a_rust_impl_method_with_a_trait_path_without_a_prefix_lists_the_trait_of_its_own_module() {
    let repo = scanned_repo(RS_RELATIVE_TRAIT_PATH);

    let method = report_at(&repo, "draw", "src/ovr.rs", 6);

    assert_eq!(
        listed(&method.overridden),
        ["Draw.draw src/ovr/a.rs:2"],
        "{}",
        dump(&method)
    );
}

const RS_UNIQUE_SUFFIX: &[(&str, &str)] = &[
    ("src/lib.rs", "pub mod ovr;\npub mod other;\n"),
    ("src/ovr.rs", "pub mod a;\npub mod b;\n"),
    (
        "src/ovr/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    (
        "src/ovr/b.rs",
        "use super::a;\n\npub struct B;\n\nimpl a::Draw for B {\n    fn draw(&self) -> i32 {\n        6\n    }\n}\n",
    ),
    (
        "src/other.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
];

#[test]
fn a_rust_trait_path_that_only_one_trait_ends_with_names_that_trait() {
    let repo = scanned_repo(RS_UNIQUE_SUFFIX);

    let in_a = report(&repo, "Draw.draw", Some("src/ovr/a.rs"));
    let other = report(&repo, "Draw.draw", Some("src/other.rs"));
    let method = report_at(&repo, "draw", "src/ovr/b.rs", 6);

    assert_eq!(
        listed(&in_a.overrides),
        ["B.draw src/ovr/b.rs:6"],
        "{}",
        dump(&in_a)
    );
    assert!(other.overrides.is_empty(), "{:?}", other.overrides);
    assert_eq!(
        listed(&method.overridden),
        ["Draw.draw src/ovr/a.rs:2"],
        "{}",
        dump(&method)
    );
}

const RS_AMBIGUOUS_SUFFIX: &[(&str, &str)] = &[
    ("src/lib.rs", "pub mod m;\npub mod n;\npub mod k;\n"),
    ("src/m.rs", "pub mod a;\n"),
    (
        "src/m/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    ("src/n.rs", "pub mod a;\n"),
    (
        "src/n/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    (
        "src/k.rs",
        "use crate::m::a;\n\npub struct K;\n\nimpl a::Draw for K {\n    fn draw(&self) -> i32 {\n        7\n    }\n}\n",
    ),
];

#[test]
fn a_rust_trait_path_that_several_traits_end_with_names_none_of_them() {
    let repo = scanned_repo(RS_AMBIGUOUS_SUFFIX);

    let in_m = report(&repo, "Draw.draw", Some("src/m/a.rs"));
    let in_n = report(&repo, "Draw.draw", Some("src/n/a.rs"));
    let method = report_at(&repo, "draw", "src/k.rs", 6);

    assert!(in_m.overrides.is_empty(), "{:?}", in_m.overrides);
    assert!(in_n.overrides.is_empty(), "{:?}", in_n.overrides);
    assert!(method.overridden.is_empty(), "{:?}", method.overridden);
}

const RS_REEXPORTED_TRAIT: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod a;\npub mod x;\npub mod y;\n\npub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct T2;\npub struct T3;\n\nimpl crate::x::Draw for T2 {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n\nimpl crate::y::Draw for T3 {\n    fn draw(&self) -> i32 {\n        3\n    }\n}\n",
    ),
    (
        "src/a.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    ("src/x.rs", "pub use crate::a::Draw;\n"),
    ("src/y.rs", "pub use crate::x::Draw;\n"),
];

#[test]
fn a_rust_trait_path_through_a_reexport_names_the_trait_it_reexports_for_one_hop() {
    let repo = scanned_repo(RS_REEXPORTED_TRAIT);

    let exported = report(&repo, "Draw.draw", Some("src/a.rs"));
    let root = report(&repo, "Draw.draw", Some("src/lib.rs"));

    assert_eq!(
        listed(&exported.overrides),
        ["T2.draw src/lib.rs:13"],
        "{}",
        dump(&exported)
    );
    assert!(root.overrides.is_empty(), "{:?}", root.overrides);
}

#[test]
fn a_rust_impl_method_with_a_trait_path_through_a_reexport_lists_the_reexported_trait() {
    let repo = scanned_repo(RS_REEXPORTED_TRAIT);

    let one_hop = report_at(&repo, "draw", "src/lib.rs", 13);
    let two_hops = report_at(&repo, "draw", "src/lib.rs", 19);

    assert_eq!(
        listed(&one_hop.overridden),
        ["Draw.draw src/a.rs:2"],
        "{}",
        dump(&one_hop)
    );
    assert!(two_hops.overridden.is_empty(), "{:?}", two_hops.overridden);
}

#[test]
fn a_rust_impl_of_a_std_trait_is_not_an_override_of_a_project_trait_of_the_same_name() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub trait Display {\n    fn fmt(&self) -> i32;\n}\n\npub struct X;\n\nimpl std::fmt::Display for X {\n    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {\n        Ok(())\n    }\n}\n",
    )]);

    let project = report(&repo, "Display.fmt", Some("src/lib.rs"));
    let std_impl = report_at(&repo, "fmt", "src/lib.rs", 8);

    assert!(project.overrides.is_empty(), "{:?}", project.overrides);
    assert!(std_impl.overridden.is_empty(), "{:?}", std_impl.overridden);
}

#[test]
fn a_rust_bare_trait_name_that_a_use_row_binds_to_std_is_not_an_override_of_a_project_trait() {
    let repo = scanned_repo(&[
        ("src/lib.rs", "pub mod mine;\npub mod out;\n"),
        (
            "src/mine.rs",
            "pub trait Write {\n    fn write(&mut self, data: &[u8]) -> usize;\n}\n\npub struct Sink;\n\nimpl Write for Sink {\n    fn write(&mut self, data: &[u8]) -> usize {\n        data.len()\n    }\n}\n",
        ),
        (
            "src/out.rs",
            "use std::io::Write;\n\npub struct Out;\n\nimpl Write for Out {\n    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {\n        Ok(data.len())\n    }\n}\n",
        ),
    ]);

    let project = report(&repo, "Write.write", Some("src/mine.rs"));
    let out = report_at(&repo, "write", "src/out.rs", 6);

    assert_eq!(
        listed(&project.overrides),
        ["Sink.write src/mine.rs:8"],
        "{}",
        dump(&project)
    );
    assert!(out.overridden.is_empty(), "{:?}", out.overridden);
}

const RS_EXTERNAL_TRAIT: &str = "pub trait Serialize {\n    fn serialize(&self) -> String;\n}\n\npub struct Mine;\n\nimpl Serialize for Mine {\n    fn serialize(&self) -> String {\n        String::new()\n    }\n}\n";

fn rs_use_serialize(source: &str) -> String {
    format!(
        "use {source}::Serialize;\n\npub struct S;\n\nimpl Serialize for S {{\n    fn serialize(&self) -> String {{\n        String::new()\n    }}\n}}\n"
    )
}

#[test]
fn a_rust_bare_trait_name_that_a_use_row_binds_to_a_dependency_is_not_an_override() {
    let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\nmy-dep = { version = \"1\" }\nrenamed = { package = \"real-name\", version = \"1\" }\ninherited.workspace = true\n\n[dev-dependencies]\ndev_only = \"1\"\n\n[target.'cfg(unix)'.dependencies]\nunix_only = \"1\"\n\n[build-dependencies.build_dep]\nversion = \"1\"\n";
    let modules = [
        ("a", "serde"),
        ("b", "my_dep"),
        ("c", "renamed"),
        ("d", "dev_only"),
        ("e", "unix_only"),
        ("f", "build_dep"),
        ("g", "inherited"),
        ("h", "undeclared"),
    ];
    let sources: Vec<(String, String)> = modules
        .iter()
        .map(|(module, source)| (format!("src/{module}.rs"), rs_use_serialize(source)))
        .collect();
    let mut files: Vec<(&str, &str)> = vec![
        ("Cargo.toml", manifest),
        (
            "src/lib.rs",
            "pub mod mine;\npub mod a;\npub mod b;\npub mod c;\npub mod d;\npub mod e;\npub mod f;\npub mod g;\npub mod h;\n",
        ),
        ("src/mine.rs", RS_EXTERNAL_TRAIT),
    ];
    files.extend(
        sources
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str())),
    );
    let repo = scanned_repo(&files);

    let project = report(&repo, "Serialize.serialize", Some("src/mine.rs"));
    let external = report_at(&repo, "serialize", "src/a.rs", 6);
    let undeclared = report_at(&repo, "serialize", "src/h.rs", 6);

    assert_eq!(
        listed(&project.overrides),
        ["S.serialize src/h.rs:6", "Mine.serialize src/mine.rs:8"],
        "{}",
        dump(&project)
    );
    assert!(external.overridden.is_empty(), "{:?}", external.overridden);
    assert_eq!(
        listed(&undeclared.overridden),
        ["Serialize.serialize src/mine.rs:2"],
        "{}",
        dump(&undeclared)
    );
}

#[test]
fn a_rust_dependency_name_that_a_project_module_package_or_crate_also_has_stays_unknown() {
    let manifest = "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = \"1\"\nlocal = \"1\"\nmy-core = { workspace = true }\nfolder-dep = \"1\"\n";
    let by_serde = rs_use_serialize("serde");
    let by_module = rs_use_serialize("local");
    let by_crate = rs_use_serialize("my_core");
    let by_folder = rs_use_serialize("folder_dep");
    let files: Vec<(&str, &str)> = vec![
        ("Cargo.toml", manifest),
        (
            "src/lib.rs",
            "pub mod mine;\npub mod local;\npub mod by_serde;\npub mod by_module;\npub mod by_crate;\npub mod by_folder;\n",
        ),
        ("src/mine.rs", RS_EXTERNAL_TRAIT),
        ("src/local.rs", "pub fn nothing() {}\n"),
        ("src/by_serde.rs", &by_serde),
        ("src/by_module.rs", &by_module),
        ("src/by_crate.rs", &by_crate),
        ("src/by_folder.rs", &by_folder),
        (
            "crates/core/Cargo.toml",
            "[package]\nname = \"my-core\"\nversion = \"0.1.0\"\n",
        ),
        ("crates/core/src/lib.rs", "pub fn nothing() {}\n"),
        ("crates/folder-dep/src/lib.rs", "pub fn nothing() {}\n"),
    ];
    let repo = scanned_repo(&files);

    let project = report(&repo, "Serialize.serialize", Some("src/mine.rs"));

    assert_eq!(
        listed(&project.overrides),
        [
            "S.serialize src/by_crate.rs:6",
            "S.serialize src/by_folder.rs:6",
            "S.serialize src/by_module.rs:6",
            "Mine.serialize src/mine.rs:8",
        ],
        "{}",
        dump(&project)
    );
}

#[test]
fn a_rust_bare_trait_name_bound_to_a_project_crate_by_a_path_dependency_is_still_read_from_that_crate()
 {
    let repo = scanned_repo(&[
        (
            "serde/Cargo.toml",
            "[package]\nname = \"serde\"\nversion = \"0.1.0\"\n",
        ),
        ("serde/src/lib.rs", RS_EXTERNAL_TRAIT),
        (
            "app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nserde = { path = \"../serde\" }\n",
        ),
        (
            "app/src/lib.rs",
            "pub mod ext;\n\npub trait Serialize {\n    fn serialize(&self) -> String;\n}\n",
        ),
        ("app/src/ext.rs", &rs_use_serialize("serde")),
    ]);

    let theirs = report(&repo, "Serialize.serialize", Some("serde/src/lib.rs"));
    let ours = report(&repo, "Serialize.serialize", Some("app/src/lib.rs"));

    assert_eq!(
        listed(&theirs.overrides),
        [
            "S.serialize app/src/ext.rs:6",
            "Mine.serialize serde/src/lib.rs:8"
        ],
        "{}",
        dump(&theirs)
    );
    assert!(ours.overrides.is_empty(), "{}", dump(&ours));
}

const RS_BARE_TRAIT: &[(&str, &str)] = &[
    (
        "src/lib.rs",
        "pub mod shapes;\npub mod other;\npub mod bare;\npub mod glob;\npub mod both;\npub mod local;\npub mod parent;\n",
    ),
    (
        "src/shapes.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    (
        "src/other.rs",
        "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
    ),
    (
        "src/bare.rs",
        "use crate::other::Draw;\n\npub struct F;\n\nimpl Draw for F {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n",
    ),
    (
        "src/glob.rs",
        "use crate::shapes::*;\n\npub struct I;\n\nimpl Draw for I {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
    ),
    (
        "src/both.rs",
        "use crate::shapes::*;\nuse crate::other::Draw;\n\npub struct B;\n\nimpl Draw for B {\n    fn draw(&self) -> i32 {\n        3\n    }\n}\n",
    ),
    (
        "src/local.rs",
        "use crate::shapes::*;\n\npub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub struct L;\n\nimpl Draw for L {\n    fn draw(&self) -> i32 {\n        4\n    }\n}\n",
    ),
    (
        "src/parent.rs",
        "use crate::other::Draw;\n\npub struct P;\n\nimpl Draw for P {\n    fn draw(&self) -> i32 {\n        5\n    }\n}\n\nmod tests {\n    use super::*;\n\n    struct T;\n\n    impl Draw for T {\n        fn draw(&self) -> i32 {\n            6\n        }\n    }\n}\n",
    ),
];

#[test]
fn a_rust_bare_trait_name_is_read_from_the_use_row_glob_or_trait_of_its_module() {
    let repo = scanned_repo(RS_BARE_TRAIT);

    let shapes = report(&repo, "Draw.draw", Some("src/shapes.rs"));
    let other = report(&repo, "Draw.draw", Some("src/other.rs"));
    let local = report(&repo, "Draw.draw", Some("src/local.rs"));

    assert_eq!(
        listed(&shapes.overrides),
        ["I.draw src/glob.rs:6"],
        "{}",
        dump(&shapes)
    );
    assert_eq!(
        listed(&other.overrides),
        [
            "F.draw src/bare.rs:6",
            "B.draw src/both.rs:7",
            "P.draw src/parent.rs:6",
            "T.draw src/parent.rs:17",
        ],
        "{}",
        dump(&other)
    );
    assert_eq!(
        listed(&local.overrides),
        ["L.draw src/local.rs:10"],
        "{}",
        dump(&local)
    );
}

#[test]
fn a_rust_impl_method_with_a_bare_trait_name_lists_the_trait_its_module_reads() {
    let repo = scanned_repo(RS_BARE_TRAIT);

    let expected = [
        ("src/bare.rs", 6, "Draw.draw src/other.rs:2"),
        ("src/glob.rs", 6, "Draw.draw src/shapes.rs:2"),
        ("src/both.rs", 7, "Draw.draw src/other.rs:2"),
        ("src/local.rs", 10, "Draw.draw src/local.rs:4"),
        ("src/parent.rs", 6, "Draw.draw src/other.rs:2"),
        ("src/parent.rs", 17, "Draw.draw src/other.rs:2"),
    ];
    for (path, line, trait_method) in expected {
        let method = report_at(&repo, "draw", path, line);
        assert_eq!(
            listed(&method.overridden),
            [trait_method],
            "{path}:{line}\n{}",
            dump(&method)
        );
    }
}

#[test]
fn a_rust_bare_trait_name_with_no_use_row_glob_or_local_trait_matches_by_name() {
    let repo = scanned_repo(&[
        (
            "src/lib.rs",
            "pub mod shapes;\npub mod other;\npub mod plain;\n",
        ),
        (
            "src/shapes.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/other.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/plain.rs",
            "pub struct N;\n\nimpl Draw for N {\n    fn draw(&self) -> i32 {\n        7\n    }\n}\n",
        ),
    ]);

    let shapes = report(&repo, "Draw.draw", Some("src/shapes.rs"));
    let other = report(&repo, "Draw.draw", Some("src/other.rs"));

    assert_eq!(
        listed(&shapes.overrides),
        ["N.draw src/plain.rs:4"],
        "{}",
        dump(&shapes)
    );
    assert_eq!(
        listed(&other.overrides),
        ["N.draw src/plain.rs:4"],
        "{}",
        dump(&other)
    );
}

#[test]
fn a_rust_bare_trait_name_whose_use_row_the_index_cannot_follow_matches_by_name() {
    let repo = scanned_repo(&[
        (
            "src/lib.rs",
            "pub mod shapes;\npub mod other;\npub mod hub;\npub mod mid;\npub mod chain;\n",
        ),
        (
            "src/shapes.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/other.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        ("src/hub.rs", "pub use crate::mid::Draw;\n"),
        ("src/mid.rs", "pub use crate::shapes::Draw;\n"),
        (
            "src/chain.rs",
            "use crate::hub::Draw;\n\npub struct C;\n\nimpl Draw for C {\n    fn draw(&self) -> i32 {\n        8\n    }\n}\n",
        ),
    ]);

    let shapes = report(&repo, "Draw.draw", Some("src/shapes.rs"));
    let other = report(&repo, "Draw.draw", Some("src/other.rs"));

    assert_eq!(
        listed(&shapes.overrides),
        ["C.draw src/chain.rs:6"],
        "{}",
        dump(&shapes)
    );
    assert_eq!(
        listed(&other.overrides),
        ["C.draw src/chain.rs:6"],
        "{}",
        dump(&other)
    );
}

#[test]
fn a_rust_leading_crate_name_is_the_package_name_of_its_cargo_toml() {
    let repo = scanned_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"my-app\"\nversion = \"0.1.0\"\n",
        ),
        ("src/lib.rs", "pub mod shapes;\n"),
        (
            "src/shapes.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "src/main.rs",
            "pub struct M;\n\nimpl my_app::shapes::Draw for M {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n",
        ),
        (
            "src/bin/tool.rs",
            "pub struct U;\n\nimpl my_app::shapes::Draw for U {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
        ),
        (
            "tests/it.rs",
            "pub struct T;\n\nimpl my_app::shapes::Draw for T {\n    fn draw(&self) -> i32 {\n        3\n    }\n}\n",
        ),
        (
            "benches/b.rs",
            "pub struct B;\n\nimpl my_app::shapes::Draw for B {\n    fn draw(&self) -> i32 {\n        4\n    }\n}\n",
        ),
        (
            "examples/e.rs",
            "pub struct E;\n\nimpl my_app::shapes::Draw for E {\n    fn draw(&self) -> i32 {\n        5\n    }\n}\n",
        ),
    ]);

    let draw = report(&repo, "Draw.draw", Some("src/shapes.rs"));
    let test = report_at(&repo, "draw", "tests/it.rs", 4);

    assert_eq!(
        listed(&draw.overrides),
        [
            "B.draw benches/b.rs:4",
            "E.draw examples/e.rs:4",
            "U.draw src/bin/tool.rs:4",
            "M.draw src/main.rs:4",
            "T.draw tests/it.rs:4",
        ],
        "{}",
        dump(&draw)
    );
    assert_eq!(
        listed(&test.overridden),
        ["Draw.draw src/shapes.rs:2"],
        "{}",
        dump(&test)
    );
}

#[test]
fn a_rust_package_name_in_cargo_toml_replaces_the_folder_name_as_the_crate_name() {
    let repo = scanned_repo(&[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/shared\", \"crates/app\"]\n",
        ),
        (
            "crates/shared/Cargo.toml",
            "[package]\nname = \"myapp-core\"\nversion = \"0.1.0\"\n",
        ),
        (
            "crates/shared/src/lib.rs",
            "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n",
        ),
        (
            "crates/app/Cargo.toml",
            "[package]\nname = \"myapp\"\nversion = \"0.1.0\"\n",
        ),
        (
            "crates/app/src/main.rs",
            "pub struct A;\n\nimpl myapp_core::Draw for A {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n\npub struct B;\n\nimpl shared::Draw for B {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n",
        ),
    ]);

    let draw = report(&repo, "Draw.draw", Some("crates/shared/src/lib.rs"));

    assert_eq!(
        listed(&draw.overrides),
        ["A.draw crates/app/src/main.rs:4"],
        "{}",
        dump(&draw)
    );
}

const PY_SAME_BASE_NAME: &[(&str, &str)] = &[
    (
        "pkg_a/base.py",
        "class Base:\n    def run(self):\n        return 1\n",
    ),
    (
        "pkg_b/base.py",
        "class Base:\n    def run(self):\n        return 2\n",
    ),
    (
        "pkg_a/sub.py",
        "from pkg_a.base import Base\n\n\nclass Sub(Base):\n    def run(self):\n        return 3\n",
    ),
    (
        "c/sub.py",
        "from pkg_b.base import Base\n\n\nclass ImportedSub(Base):\n    def run(self):\n        return 4\n",
    ),
];

#[test]
fn a_python_subclass_that_imports_another_base_of_the_same_name_is_not_an_override() {
    let repo = scanned_repo(PY_SAME_BASE_NAME);

    let near = first_definition_report(&repo, "run", "pkg_a/base.py");
    let far = first_definition_report(&repo, "run", "pkg_b/base.py");

    assert_eq!(listed(&near.overrides), ["Sub.run pkg_a/sub.py:5"]);
    assert_eq!(listed(&far.overrides), ["ImportedSub.run c/sub.py:5"]);
}

#[test]
fn a_python_method_lists_only_the_base_its_import_names() {
    let repo = scanned_repo(PY_SAME_BASE_NAME);

    let imported = first_definition_report(&repo, "run", "c/sub.py");
    let near = first_definition_report(&repo, "run", "pkg_a/sub.py");

    assert_eq!(listed(&imported.overridden), ["Base.run pkg_b/base.py:2"]);
    assert_eq!(listed(&near.overridden), ["Base.run pkg_a/base.py:2"]);
}

#[test]
fn an_import_of_a_far_base_wins_over_a_nearer_base_of_the_same_name() {
    let repo = scanned_repo(&[
        (
            "a/base.py",
            "class Base:\n    def run(self):\n        return 1\n",
        ),
        (
            "b/base.py",
            "class Base:\n    def run(self):\n        return 2\n",
        ),
        (
            "b/sub.py",
            "from a.base import Base\n\n\nclass Sub(Base):\n    def run(self):\n        return 3\n",
        ),
    ]);

    let far = first_definition_report(&repo, "run", "a/base.py");
    let near = first_definition_report(&repo, "run", "b/base.py");
    let sub = first_definition_report(&repo, "run", "b/sub.py");

    assert_eq!(
        listed(&far.overrides),
        ["Sub.run b/sub.py:5"],
        "{}",
        dump(&far)
    );
    assert!(near.overrides.is_empty(), "{:?}", near.overrides);
    assert_eq!(listed(&sub.overridden), ["Base.run a/base.py:2"]);
}

const CROSS_LANGUAGE_BASES: &[(&str, &str)] = &[
    (
        "pkg_b/base.py",
        "class Base:\n    def run(self):\n        return 2\n",
    ),
    (
        "c/sub.py",
        "from pkg_b.base import Base\n\n\nclass ImportedSub(Base):\n    def run(self):\n        return 4\n",
    ),
    (
        "web/base.ts",
        "export class Base {\n  run(): number {\n    return 1;\n  }\n}\n",
    ),
    (
        "web/derived.ts",
        "import { Base } from './base';\n\nexport class Derived extends Base {\n  run(): number {\n    return 2;\n  }\n}\n",
    ),
];

#[test]
fn a_subclass_in_another_language_is_not_an_override() {
    let repo = scanned_repo(CROSS_LANGUAGE_BASES);

    let python = first_definition_report(&repo, "run", "pkg_b/base.py");
    let typescript = first_definition_report(&repo, "run", "web/base.ts");

    assert_eq!(listed(&python.overrides), ["ImportedSub.run c/sub.py:5"]);
    assert_eq!(
        listed(&typescript.overrides),
        ["Derived.run web/derived.ts:4"]
    );
}

#[test]
fn a_base_class_in_another_language_is_not_listed_as_overridden() {
    let repo = scanned_repo(CROSS_LANGUAGE_BASES);

    let python = first_definition_report(&repo, "run", "c/sub.py");
    let typescript = first_definition_report(&repo, "run", "web/derived.ts");

    assert_eq!(listed(&python.overridden), ["Base.run pkg_b/base.py:2"]);
    assert_eq!(listed(&typescript.overridden), ["Base.run web/base.ts:2"]);
}

const JAVA_SAME_BASE_NAME: &[(&str, &str)] = &[
    (
        "a/Base.java",
        "package a;\n\npublic class Base {\n    public int run(int x) {\n        return x;\n    }\n}\n",
    ),
    (
        "b/Base.java",
        "package b;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
    ),
    (
        "a/Kid.java",
        "package a;\n\npublic class Kid extends Base {\n    public int run(int x) {\n        return x + 1;\n    }\n}\n",
    ),
    (
        "c/Third.java",
        "package c;\n\nimport b.Base;\n\npublic class Third extends Base {\n    public int run(int x) {\n        return x * 3;\n    }\n}\n",
    ),
];

#[test]
fn a_java_subclass_that_imports_another_base_of_the_same_name_is_not_an_override() {
    let repo = scanned_repo(JAVA_SAME_BASE_NAME);

    let a = first_definition_report(&repo, "run", "a/Base.java");
    let b = first_definition_report(&repo, "run", "b/Base.java");

    assert_eq!(
        listed(&a.overrides),
        ["Kid.run a/Kid.java:4"],
        "{}",
        dump(&a)
    );
    assert_eq!(
        listed(&b.overrides),
        ["Third.run c/Third.java:6"],
        "{}",
        dump(&b)
    );
}

#[test]
fn a_java_method_lists_only_the_base_its_import_names() {
    let repo = scanned_repo(JAVA_SAME_BASE_NAME);

    let third = first_definition_report(&repo, "run", "c/Third.java");
    let kid = first_definition_report(&repo, "run", "a/Kid.java");

    assert_eq!(listed(&third.overridden), ["Base.run b/Base.java:4"]);
    assert_eq!(listed(&kid.overridden), ["Base.run a/Base.java:4"]);
}

const RS_ENUM_AND_UNION_IMPLS: &str = "pub trait Draw {\n    fn draw(&self) -> i32;\n}\n\npub enum Color {\n    Red,\n}\n\nimpl Draw for Color {\n    fn draw(&self) -> i32 {\n        1\n    }\n}\n\npub union Bits {\n    pub a: u32,\n}\n\nimpl Draw for Bits {\n    fn draw(&self) -> i32 {\n        2\n    }\n}\n\npub struct Dot;\n\nimpl Draw for Dot {\n    fn draw(&self) -> i32 {\n        3\n    }\n}\n";

#[test]
fn a_rust_enum_and_union_that_implement_a_trait_override_its_methods() {
    let repo = scanned_repo(&[("src/lib.rs", RS_ENUM_AND_UNION_IMPLS)]);

    let draw = report(&repo, "Draw.draw", None);
    let color = report_at(&repo, "draw", "src/lib.rs", 10);
    let bits = report_at(&repo, "draw", "src/lib.rs", 20);

    assert_eq!(
        listed(&draw.overrides),
        [
            "Color.draw src/lib.rs:10",
            "Bits.draw src/lib.rs:20",
            "Dot.draw src/lib.rs:28"
        ],
        "{}",
        dump(&draw)
    );
    assert_eq!(listed(&color.overridden), ["Draw.draw src/lib.rs:2"]);
    assert_eq!(listed(&bits.overridden), ["Draw.draw src/lib.rs:2"]);
}

const JAVA_ENUM_IMPLEMENTS: &[(&str, &str)] = &[
    (
        "a/Handler.java",
        "package a;\n\npublic interface Handler {\n    void run();\n}\n",
    ),
    (
        "b/Handler.java",
        "package b;\n\npublic interface Handler {\n    void run();\n}\n",
    ),
    (
        "p/E.java",
        "package p;\n\nimport b.Handler;\n\npublic enum E implements Handler {\n    X;\n    public void run() {}\n}\n",
    ),
];

#[test]
fn a_java_enum_that_implements_an_interface_overrides_the_interface_it_imports() {
    let repo = scanned_repo(JAVA_ENUM_IMPLEMENTS);

    let imported = first_definition_report(&repo, "run", "b/Handler.java");
    let other = first_definition_report(&repo, "run", "a/Handler.java");
    let method = first_definition_report(&repo, "run", "p/E.java");

    assert_eq!(
        listed(&imported.overrides),
        ["E.run p/E.java:7"],
        "{}",
        dump(&imported)
    );
    assert!(other.overrides.is_empty(), "{}", dump(&other));
    assert_eq!(listed(&method.overridden), ["Handler.run b/Handler.java:4"]);
}

const KOTLIN_ENUM_IMPLEMENTS: &[(&str, &str)] = &[
    ("Handler.kt", "interface Handler {\n    fun run(): Int\n}\n"),
    (
        "Color.kt",
        "enum class Color : Handler {\n    RED;\n    override fun run(): Int = 1\n}\n",
    ),
];

#[test]
fn a_kotlin_enum_class_that_implements_an_interface_overrides_its_methods() {
    let repo = scanned_repo(KOTLIN_ENUM_IMPLEMENTS);

    let handler = report(&repo, "Handler.run", None);

    assert_eq!(
        listed(&handler.overrides),
        ["Color.run Color.kt:3"],
        "{}",
        dump(&handler)
    );
}

const PY_ENUM_EXTENDS: &[(&str, &str)] = &[
    (
        "pkg/mixin.py",
        "class Describes:\n    def describe(self):\n        return 'base'\n",
    ),
    (
        "pkg/kinds.py",
        "from enum import Enum\n\nfrom pkg.mixin import Describes\n\n\nclass Shade(Describes, Enum):\n    LIGHT = 1\n\n    def describe(self):\n        return 'shade'\n\n\nclass Tone(Enum):\n    def tell(self):\n        return 'tone'\n\n\nclass Pitch(Tone):\n    def tell(self):\n        return 'pitch'\n",
    ),
];

#[test]
fn a_python_enum_can_extend_a_class_and_be_extended_when_it_has_no_members() {
    let repo = scanned_repo(PY_ENUM_EXTENDS);

    let describes = report(&repo, "Describes.describe", None);
    let tone = report(&repo, "Tone.tell", None);
    let pitch = report(&repo, "Pitch.tell", None);

    assert_eq!(
        listed(&describes.overrides),
        ["Shade.describe pkg/kinds.py:9"],
        "{}",
        dump(&describes)
    );
    assert_eq!(
        listed(&tone.overrides),
        ["Pitch.tell pkg/kinds.py:19"],
        "{}",
        dump(&tone)
    );
    assert_eq!(listed(&pitch.overridden), ["Tone.tell pkg/kinds.py:14"]);
}

const JAVA_SUB_PACKAGES: &[(&str, &str)] = &[
    (
        "b/Base.java",
        "package b;\n\npublic class Base {\n    public int run(int x) {\n        return x;\n    }\n}\n",
    ),
    (
        "b/x/Base.java",
        "package b.x;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
    ),
    (
        "p/K1.java",
        "package p;\n\nimport b.Base;\n\npublic class K1 extends Base {\n    public int run(int x) {\n        return 1;\n    }\n}\n",
    ),
    (
        "q/K11.java",
        "package q;\n\nimport b.x.Base;\n\npublic class K11 extends Base {\n    public int run(int x) {\n        return 11;\n    }\n}\n",
    ),
];

#[test]
fn a_java_import_of_a_sub_package_class_does_not_tie_the_class_of_the_parent_package() {
    let repo = scanned_repo(JAVA_SUB_PACKAGES);

    let parent = first_definition_report(&repo, "run", "b/Base.java");
    let child = first_definition_report(&repo, "run", "b/x/Base.java");

    assert_eq!(
        listed(&parent.overrides),
        ["K1.run p/K1.java:6"],
        "{}",
        dump(&parent)
    );
    assert_eq!(
        listed(&child.overrides),
        ["K11.run q/K11.java:6"],
        "{}",
        dump(&child)
    );
}

#[test]
fn a_java_method_lists_only_the_sub_package_base_its_import_names() {
    let repo = scanned_repo(JAVA_SUB_PACKAGES);

    let nested = first_definition_report(&repo, "run", "q/K11.java");
    let plain = first_definition_report(&repo, "run", "p/K1.java");

    assert_eq!(listed(&nested.overridden), ["Base.run b/x/Base.java:4"]);
    assert_eq!(listed(&plain.overridden), ["Base.run b/Base.java:4"]);
}

const JAVA_NESTED_BASES: &[(&str, &str)] = &[
    (
        "b/Inner.java",
        "package b;\n\npublic class Inner {\n    public int run(int x) {\n        return x;\n    }\n}\n",
    ),
    (
        "b/Outer.java",
        "package b;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
    ),
    (
        "c/Outer.java",
        "package c;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
    ),
    (
        "q/K14.java",
        "package q;\n\nimport b.Outer.Inner;\n\npublic class K14 extends Inner {\n    public int run(int x) {\n        return 14;\n    }\n}\n",
    ),
];

#[test]
fn a_java_import_of_a_nested_class_ties_only_the_class_nested_in_the_named_type() {
    let repo = scanned_repo(JAVA_NESTED_BASES);

    let top_level = first_definition_report(&repo, "run", "b/Inner.java");
    let nested = first_definition_report(&repo, "run", "b/Outer.java");
    let other = first_definition_report(&repo, "run", "c/Outer.java");

    assert!(top_level.overrides.is_empty(), "{:?}", top_level.overrides);
    assert_eq!(
        listed(&nested.overrides),
        ["K14.run q/K14.java:6"],
        "{}",
        dump(&nested)
    );
    assert!(other.overrides.is_empty(), "{:?}", other.overrides);
}

const JAVA_WRITTEN_PACKAGES: &[(&str, &str)] = &[
    (
        "a/Base.java",
        "package a;\n\npublic class Base {\n    public int run(int x) {\n        return x;\n    }\n}\n",
    ),
    (
        "b/Base.java",
        "package b;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
    ),
    (
        "c/Base.java",
        "package c;\n\npublic class Base {\n    public int run(int x) {\n        return 0;\n    }\n}\n",
    ),
    (
        "b/x/Base.java",
        "package b.x;\n\npublic class Base {\n    public int run(int x) {\n        return 1;\n    }\n}\n",
    ),
    (
        "p/K3.java",
        "package p;\n\npublic class K3 extends b.Base {\n    public int run(int x) {\n        return 3;\n    }\n}\n",
    ),
    (
        "p/K4.java",
        "package p;\n\nimport b.Base;\n\npublic class K4 extends c.Base {\n    public int run(int x) {\n        return 4;\n    }\n}\n",
    ),
    (
        "p/K12.java",
        "package p;\n\npublic class K12 extends b.x.Base {\n    public int run(int x) {\n        return 12;\n    }\n}\n",
    ),
];

#[test]
fn a_java_base_written_with_its_package_is_listed_only_under_that_package() {
    let repo = scanned_repo(JAVA_WRITTEN_PACKAGES);

    let a = first_definition_report(&repo, "run", "a/Base.java");
    let b = first_definition_report(&repo, "run", "b/Base.java");
    let deep = first_definition_report(&repo, "run", "b/x/Base.java");

    assert!(a.overrides.is_empty(), "{:?}", a.overrides);
    assert_eq!(listed(&b.overrides), ["K3.run p/K3.java:4"], "{}", dump(&b));
    assert_eq!(
        listed(&deep.overrides),
        ["K12.run p/K12.java:4"],
        "{}",
        dump(&deep)
    );
}

#[test]
fn a_java_package_written_before_the_base_wins_over_an_import_of_its_name() {
    let repo = scanned_repo(JAVA_WRITTEN_PACKAGES);

    let imported = first_definition_report(&repo, "run", "b/Base.java");
    let written = first_definition_report(&repo, "run", "c/Base.java");

    assert_eq!(
        listed(&written.overrides),
        ["K4.run p/K4.java:6"],
        "{}",
        dump(&written)
    );
    assert!(
        !listed(&imported.overrides).contains(&"K4.run p/K4.java:6".to_string()),
        "{:?}",
        imported.overrides
    );
}

#[test]
fn a_java_method_lists_only_the_base_written_with_its_package() {
    let repo = scanned_repo(JAVA_WRITTEN_PACKAGES);

    let plain = first_definition_report(&repo, "run", "p/K3.java");
    let deep = first_definition_report(&repo, "run", "p/K12.java");
    let imported_elsewhere = first_definition_report(&repo, "run", "p/K4.java");

    assert_eq!(listed(&plain.overridden), ["Base.run b/Base.java:4"]);
    assert_eq!(listed(&deep.overridden), ["Base.run b/x/Base.java:4"]);
    assert_eq!(
        listed(&imported_elsewhere.overridden),
        ["Base.run c/Base.java:4"]
    );
}

const JAVA_WRITTEN_OUTER: &[(&str, &str)] = &[
    (
        "b/Outer.java",
        "package b;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
    ),
    (
        "c/Outer.java",
        "package c;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
    ),
    (
        "q/K13.java",
        "package q;\n\nimport b.Outer;\n\npublic class K13 extends Outer.Inner {\n    public int run(int x) {\n        return 13;\n    }\n}\n",
    ),
    (
        "q/K15.java",
        "package q;\n\npublic class K15 extends c.Outer.Inner {\n    public int run(int x) {\n        return 15;\n    }\n}\n",
    ),
];

#[test]
fn a_java_base_written_with_its_outer_type_is_listed_only_under_the_type_that_name_reaches() {
    let repo = scanned_repo(JAVA_WRITTEN_OUTER);

    let b = first_definition_report(&repo, "run", "b/Outer.java");
    let c = first_definition_report(&repo, "run", "c/Outer.java");

    assert_eq!(
        listed(&b.overrides),
        ["K13.run q/K13.java:6"],
        "{}",
        dump(&b)
    );
    assert_eq!(
        listed(&c.overrides),
        ["K15.run q/K15.java:4"],
        "{}",
        dump(&c)
    );
}

const JAVA_STAR_IMPORTS: &[(&str, &str)] = &[
    (
        "a/Base.java",
        "package a;\n\npublic class Base {\n    public int run(int x) {\n        return x;\n    }\n}\n",
    ),
    (
        "b/Base.java",
        "package b;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
    ),
    (
        "c/Base.java",
        "package c;\n\npublic class Base {\n    public int run(int x) {\n        return 0;\n    }\n}\n",
    ),
    (
        "q/K2.java",
        "package q;\n\nimport b.*;\n\npublic class K2 extends Base {\n    public int run(int x) {\n        return 2;\n    }\n}\n",
    ),
    (
        "q/K18.java",
        "package q;\n\nimport c.*;\nimport b.Base;\n\npublic class K18 extends Base {\n    public int run(int x) {\n        return 18;\n    }\n}\n",
    ),
];

#[test]
fn a_java_star_import_ties_the_base_only_to_the_package_it_names() {
    let repo = scanned_repo(JAVA_STAR_IMPORTS);

    let a = first_definition_report(&repo, "run", "a/Base.java");
    let b = first_definition_report(&repo, "run", "b/Base.java");
    let c = first_definition_report(&repo, "run", "c/Base.java");

    assert!(a.overrides.is_empty(), "{:?}", a.overrides);
    assert!(
        listed(&b.overrides).contains(&"K2.run q/K2.java:6".to_string()),
        "{}",
        dump(&b)
    );
    assert!(
        !listed(&c.overrides).contains(&"K2.run q/K2.java:6".to_string()),
        "{:?}",
        c.overrides
    );
}

#[test]
fn a_java_explicit_import_wins_over_a_star_import_of_another_package() {
    let repo = scanned_repo(JAVA_STAR_IMPORTS);

    let b = first_definition_report(&repo, "run", "b/Base.java");
    let c = first_definition_report(&repo, "run", "c/Base.java");

    assert_eq!(
        listed(&b.overrides),
        ["K18.run q/K18.java:7", "K2.run q/K2.java:6"],
        "{}",
        dump(&b)
    );
    assert!(c.overrides.is_empty(), "{:?}", c.overrides);
}

#[test]
fn a_java_method_lists_only_the_base_in_the_package_its_star_import_names() {
    let repo = scanned_repo(JAVA_STAR_IMPORTS);

    let star = first_definition_report(&repo, "run", "q/K2.java");

    assert_eq!(listed(&star.overridden), ["Base.run b/Base.java:4"]);
}

#[test]
fn a_java_star_import_in_a_block_comment_or_a_text_block_is_not_read() {
    let repo = scanned_repo(&[
        (
            "b/Base.java",
            "package b;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
        ),
        (
            "c/Base.java",
            "package c;\n\npublic class Base {\n    public int run(int x) {\n        return 0;\n    }\n}\n",
        ),
        (
            "q/K20.java",
            "package q;\n\n/*\nimport b.*;\n*/\nimport c.*;\n\npublic class K20 extends Base {\n    public int run(int x) {\n        return 20;\n    }\n}\n",
        ),
        (
            "q/K21.java",
            "package q;\n\nimport c.*;\n\npublic class K21 extends Base {\n    static final String T = \"\"\"\nimport b.*;\n\"\"\";\n\n    public int run(int x) {\n        return 21;\n    }\n}\n",
        ),
    ]);

    let b = first_definition_report(&repo, "run", "b/Base.java");
    let c = first_definition_report(&repo, "run", "c/Base.java");

    assert!(b.overrides.is_empty(), "{:?}", b.overrides);
    assert_eq!(
        listed(&c.overrides),
        ["K20.run q/K20.java:9", "K21.run q/K21.java:10"],
        "{}",
        dump(&c)
    );
}

#[test]
fn a_java_class_of_the_same_package_wins_over_a_star_import() {
    let repo = scanned_repo(&[
        (
            "b/Base.java",
            "package b;\n\npublic class Base {\n    public int run(int x) {\n        return -x;\n    }\n}\n",
        ),
        (
            "p/Base.java",
            "package p;\n\npublic class Base {\n    public int run(int x) {\n        return x;\n    }\n}\n",
        ),
        (
            "p/K16.java",
            "package p;\n\nimport b.*;\n\npublic class K16 extends Base {\n    public int run(int x) {\n        return 16;\n    }\n}\n",
        ),
    ]);

    let imported_package = first_definition_report(&repo, "run", "b/Base.java");
    let same_package = first_definition_report(&repo, "run", "p/Base.java");
    let sub = first_definition_report(&repo, "run", "p/K16.java");

    assert!(
        imported_package.overrides.is_empty(),
        "{:?}",
        imported_package.overrides
    );
    assert_eq!(
        listed(&same_package.overrides),
        ["K16.run p/K16.java:6"],
        "{}",
        dump(&same_package)
    );
    assert_eq!(listed(&sub.overridden), ["Base.run p/Base.java:4"]);
}

#[test]
fn a_java_star_import_of_an_outer_type_ties_the_class_nested_in_it() {
    let repo = scanned_repo(&[
        (
            "b/Outer.java",
            "package b;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
        ),
        (
            "c/Outer.java",
            "package c;\n\npublic class Outer {\n    public static class Inner {\n        public int run(int x) {\n            return x;\n        }\n    }\n}\n",
        ),
        (
            "q/K17.java",
            "package q;\n\nimport b.Outer.*;\n\npublic class K17 extends Inner {\n    public int run(int x) {\n        return 17;\n    }\n}\n",
        ),
    ]);

    let b = first_definition_report(&repo, "run", "b/Outer.java");
    let c = first_definition_report(&repo, "run", "c/Outer.java");

    assert_eq!(
        listed(&b.overrides),
        ["K17.run q/K17.java:6"],
        "{}",
        dump(&b)
    );
    assert!(c.overrides.is_empty(), "{:?}", c.overrides);
}

#[test]
fn a_kotlin_base_is_tied_by_its_written_package_its_import_or_its_star_import() {
    let repo = scanned_repo(&[
        (
            "b/Base.kt",
            "package b\n\nopen class Base {\n    open fun run(x: Int): Int {\n        return x\n    }\n}\n",
        ),
        (
            "c/Base.kt",
            "package c\n\nopen class Base {\n    open fun run(x: Int): Int {\n        return x\n    }\n}\n",
        ),
        (
            "p/K1.kt",
            "package p\n\nclass K1 : b.Base() {\n    override fun run(x: Int): Int {\n        return 1\n    }\n}\n",
        ),
        (
            "p/K2.kt",
            "package p\n\nimport b.Base\nimport c.*\n\nclass K2 : Base() {\n    override fun run(x: Int): Int {\n        return 2\n    }\n}\n",
        ),
        (
            "p/K3.kt",
            "package p\n\nimport c.*\n\nclass K3 : Base() {\n    override fun run(x: Int): Int {\n        return 3\n    }\n}\n",
        ),
    ]);

    let b = first_definition_report(&repo, "run", "b/Base.kt");
    let c = first_definition_report(&repo, "run", "c/Base.kt");

    assert_eq!(
        listed(&b.overrides),
        ["K1.run p/K1.kt:4", "K2.run p/K2.kt:7"],
        "{}",
        dump(&b)
    );
    assert_eq!(listed(&c.overrides), ["K3.run p/K3.kt:6"], "{}", dump(&c));
}

#[test]
fn a_scala_base_is_tied_by_its_written_package_its_import_or_its_star_import() {
    let repo = scanned_repo(&[
        (
            "b/Base.scala",
            "package b\n\nclass Base {\n  def run(x: Int): Int = x\n}\n",
        ),
        (
            "c/Base.scala",
            "package c\n\nclass Base {\n  def run(x: Int): Int = x\n}\n",
        ),
        (
            "p/K1.scala",
            "package p\n\nclass K1 extends b.Base {\n  override def run(x: Int): Int = 1\n}\n",
        ),
        (
            "p/K2.scala",
            "package p\n\nimport b.Base\nimport c._\n\nclass K2 extends Base {\n  override def run(x: Int): Int = 2\n}\n",
        ),
        (
            "p/K3.scala",
            "package p\n\nimport c._\n\nclass K3 extends Base {\n  override def run(x: Int): Int = 3\n}\n",
        ),
    ]);

    let b = first_definition_report(&repo, "run", "b/Base.scala");
    let c = first_definition_report(&repo, "run", "c/Base.scala");

    assert_eq!(
        listed(&b.overrides),
        ["K1.run p/K1.scala:4", "K2.run p/K2.scala:7"],
        "{}",
        dump(&b)
    );
    assert_eq!(
        listed(&c.overrides),
        ["K3.run p/K3.scala:6"],
        "{}",
        dump(&c)
    );
}

const PY_TWO_BASES: &[(&str, &str)] = &[
    ("pkg_a/__init__.py", ""),
    (
        "pkg_a/base.py",
        "class Base:\n    def run(self, x):\n        return x\n",
    ),
    ("pkg_b/__init__.py", ""),
    (
        "pkg_b/base.py",
        "class Base:\n    def run(self, x):\n        return -x\n",
    ),
    ("apps/__init__.py", ""),
    (
        "apps/s4.py",
        "from pkg_a.base import Base\n\n\nclass S4(Base):\n    def run(self, x):\n        return 4\n",
    ),
];

#[test]
fn a_python_base_imported_from_a_module_outside_the_project_ties_no_project_class() {
    let files = [
        PY_TWO_BASES,
        &[(
            "apps/s9.py",
            "from external_lib import Base\n\n\nclass S9(Base):\n    def run(self, x):\n        return 9\n",
        )],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let a = first_definition_report(&repo, "run", "pkg_a/base.py");
    let b = first_definition_report(&repo, "run", "pkg_b/base.py");
    let sub = first_definition_report(&repo, "run", "apps/s9.py");

    assert_eq!(
        listed(&a.overrides),
        ["S4.run apps/s4.py:5"],
        "{}",
        dump(&a)
    );
    assert!(b.overrides.is_empty(), "{:?}", b.overrides);
    assert!(sub.overridden.is_empty(), "{:?}", sub.overridden);
}

#[test]
fn a_python_base_written_with_its_module_path_is_listed_only_under_that_module() {
    let files = [
        PY_TWO_BASES,
        &[
            (
                "apps/s5.py",
                "import pkg_a.base\n\n\nclass S5(pkg_a.base.Base):\n    def run(self, x):\n        return 5\n",
            ),
            (
                "apps/t5.py",
                "import pkg_b.base\n\n\nclass T5(pkg_b.base.Base):\n    def run(self, x):\n        return 15\n",
            ),
        ],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let a = first_definition_report(&repo, "run", "pkg_a/base.py");
    let b = first_definition_report(&repo, "run", "pkg_b/base.py");

    assert_eq!(
        listed(&a.overrides),
        ["S4.run apps/s4.py:5", "S5.run apps/s5.py:5"],
        "{}",
        dump(&a)
    );
    assert_eq!(
        listed(&b.overrides),
        ["T5.run apps/t5.py:5"],
        "{}",
        dump(&b)
    );
}

#[test]
fn a_python_base_written_through_a_module_alias_is_listed_only_under_that_module() {
    let files = [
        PY_TWO_BASES,
        &[
            (
                "apps/s1.py",
                "import pkg_b.base as pb\n\n\nclass S1(pb.Base):\n    def run(self, x):\n        return 1\n",
            ),
            (
                "apps/s2.py",
                "from pkg_b import base\n\n\nclass S2(base.Base):\n    def run(self, x):\n        return 2\n",
            ),
        ],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let a = first_definition_report(&repo, "run", "pkg_a/base.py");
    let b = first_definition_report(&repo, "run", "pkg_b/base.py");

    assert_eq!(
        listed(&a.overrides),
        ["S4.run apps/s4.py:5"],
        "{}",
        dump(&a)
    );
    assert_eq!(
        listed(&b.overrides),
        ["S1.run apps/s1.py:5", "S2.run apps/s2.py:5"],
        "{}",
        dump(&b)
    );
}

#[test]
fn a_python_method_lists_only_the_base_written_with_its_module_path() {
    let files = [
        PY_TWO_BASES,
        &[(
            "apps/s5.py",
            "import pkg_a.base\n\n\nclass S5(pkg_a.base.Base):\n    def run(self, x):\n        return 5\n",
        )],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let sub = first_definition_report(&repo, "run", "apps/s5.py");

    assert_eq!(listed(&sub.overridden), ["Base.run pkg_a/base.py:2"]);
}

#[test]
fn a_python_star_import_ties_the_base_only_to_the_module_or_package_it_names() {
    let files = [
        PY_TWO_BASES,
        &[
            (
                "apps/s6.py",
                "from pkg_b.base import *\n\n\nclass S6(Base):\n    def run(self, x):\n        return 6\n",
            ),
            (
                "apps/t6.py",
                "from pkg_a.base import *\n\n\nclass T6(Base):\n    def run(self, x):\n        return 16\n",
            ),
            (
                "apps/u6.py",
                "from pkg_b import *\n\n\nclass U6(Base):\n    def run(self, x):\n        return 26\n",
            ),
        ],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let a = first_definition_report(&repo, "run", "pkg_a/base.py");
    let b = first_definition_report(&repo, "run", "pkg_b/base.py");
    let s6 = first_definition_report(&repo, "run", "apps/s6.py");

    assert_eq!(
        listed(&a.overrides),
        ["S4.run apps/s4.py:5", "T6.run apps/t6.py:5"],
        "{}",
        dump(&a)
    );
    assert_eq!(
        listed(&b.overrides),
        ["S6.run apps/s6.py:5", "U6.run apps/u6.py:5"],
        "{}",
        dump(&b)
    );
    assert_eq!(listed(&s6.overridden), ["Base.run pkg_b/base.py:2"]);
}

#[test]
fn an_explicit_python_import_wins_over_a_star_import_of_another_module() {
    let files = [
        PY_TWO_BASES,
        &[(
            "apps/v6.py",
            "from pkg_a.base import Base\nfrom pkg_b.base import *\n\n\nclass V6(Base):\n    def run(self, x):\n        return 36\n",
        )],
    ]
    .concat();
    let repo = scanned_repo(&files);

    let a = first_definition_report(&repo, "run", "pkg_a/base.py");
    let b = first_definition_report(&repo, "run", "pkg_b/base.py");

    assert_eq!(
        listed(&a.overrides),
        ["S4.run apps/s4.py:5", "V6.run apps/v6.py:6"],
        "{}",
        dump(&a)
    );
    assert!(b.overrides.is_empty(), "{:?}", b.overrides);
}

#[test]
fn a_constructor_lists_no_overrides() {
    let repo = scanned_repo(&[(
        "pkg/base.py",
        "class Base:\n    def __init__(self):\n        self.a = 1\n\n\nclass Sub(Base):\n    def __init__(self):\n        super().__init__()\n",
    )]);

    let report = report(&repo, "Base.__init__", None);

    assert!(report.overrides.is_empty() && report.overridden.is_empty());
}

#[test]
fn the_subtype_walk_stops_at_fifty_types_and_says_so() {
    let mut text = String::from("class Base:\n    def run(self):\n        return 0\n");
    for n in 0..52 {
        text.push_str(&format!(
            "\n\nclass S{n:02}(Base):\n    def run(self):\n        return {n}\n"
        ));
    }
    let repo = scanned_repo(&[("pkg/many.py", text.as_str())]);

    let report = report(&repo, "Base.run", None);

    assert_eq!(report.overrides.len(), 50);
    assert!(
        report
            .cannot_see
            .iter()
            .any(|note| note.contains("first 50 subtypes of `Base`")),
        "{:?}",
        report.cannot_see
    );
}

#[test]
fn the_dispatch_note_is_kept_only_for_go_targets() {
    let go = report(&scanned_repo(GO), "Use", Some("svc/b.go"));
    assert_eq!(
        go.cannot_see,
        vec![
            "calls through a Go interface that the index does not record reach methods this list does not show"
        ]
    );

    for (language, repo, name) in [
        ("python", scanned_repo(PY_OVERRIDES), "Base.render"),
        ("java", scanned_repo(JAVA), "Util.help"),
    ] {
        let report = report(&repo, name, None);
        assert!(
            report.cannot_see.iter().all(
                |note| !note.contains("dispatch") && !note.contains("interface that the index")
            ),
            "{language}: {:?}",
            report.cannot_see
        );
    }
}

const RS_STRUCTS: &str = r#"pub struct Foo {
    pub a: i32,
}

pub struct Wrapper<T> {
    pub v: T,
}

pub trait Tr {
    fn t(&self);
}

impl Foo {
    pub fn plain() -> Foo {
        Foo { a: 1 }
    }
}

impl Tr for Foo {
    fn t(&self) {}
}

impl<T> Wrapper<T> {
    pub fn get(&self) -> &T {
        &self.v
    }
}

pub fn by_ref() -> i32 {
    let f = &Foo { a: 2 };
    f.a
}

pub fn generic() -> Wrapper<i32> {
    Wrapper::<i32> { v: 1 }
}

pub fn destructure(x: Foo) -> i32 {
    let Foo { a } = x;
    match x {
        Foo { a: 7 } => 1,
        _ => a,
    }
}

pub fn destructure_across_lines(x: Foo) -> i32 {
    let Foo {
        a,
    } = x;
    a
}

pub fn make() -> Foo {
    Foo { a: 3 }
}

pub fn make_across_lines() -> Foo {
    let made = Foo {
        a: 9,
    };
    made
}

pub struct Holder {
    pub inner: Foo,
}

pub fn nested() -> (Holder, Vec<Foo>) {
    (
        Holder {
            inner: Foo { a: 11 },
        },
        vec![Foo { a: 12 }],
    )
}
"#;

fn constructed(report: &ReferenceReport) -> Vec<String> {
    report
        .constructions
        .iter()
        .map(|site| format!("{}:{}", site.path, site.line))
        .collect()
}

fn at(repo: &Repo, path: &str, needle: &str) -> String {
    format!("{path}:{}", line_of(repo, path, needle))
}

#[test]
fn rust_struct_literals_are_constructions_and_type_positions_before_a_brace_are_not() {
    let repo = scanned_repo(&[("src/lib.rs", RS_STRUCTS)]);

    let foo = report(&repo, "Foo", None);

    assert_eq!(
        constructed(&foo),
        [
            at(&repo, "src/lib.rs", "Foo { a: 1 }"),
            at(&repo, "src/lib.rs", "&Foo { a: 2 }"),
            at(&repo, "src/lib.rs", "Foo { a: 3 }"),
            at(&repo, "src/lib.rs", "let made = Foo {"),
            at(&repo, "src/lib.rs", "inner: Foo { a: 11 }"),
            at(&repo, "src/lib.rs", "vec![Foo { a: 12 }]"),
        ],
        "{}",
        dump(&foo)
    );
}

#[test]
fn a_rust_generic_struct_counts_a_turbofish_literal_and_not_its_impl_or_return_type() {
    let repo = scanned_repo(&[("src/lib.rs", RS_STRUCTS)]);

    let wrapper = report(&repo, "Wrapper", None);

    assert_eq!(
        constructed(&wrapper),
        [at(&repo, "src/lib.rs", "Wrapper::<i32> { v: 1 }")],
        "{}",
        dump(&wrapper)
    );
}

#[test]
fn a_rust_path_qualified_struct_literal_is_a_construction_and_its_return_type_is_not() {
    let repo = scanned_repo(&[
        (
            "src/lib.rs",
            "pub mod shapes;\n\npub fn top() -> shapes::Foo {\n    shapes::Foo { a: 5 }\n}\n",
        ),
        ("src/shapes.rs", "pub struct Foo {\n    pub a: i32,\n}\n"),
    ]);

    let foo = report(&repo, "Foo", Some("src/shapes.rs"));

    assert_eq!(
        constructed(&foo),
        [at(&repo, "src/lib.rs", "shapes::Foo { a: 5 }")],
        "{}",
        dump(&foo)
    );
}

const RS_PATTERN_LIB: &str =
    "pub mod pats;\npub mod shapes;\n\npub struct Foo {\n    pub a: i32,\n    pub b: i32,\n}\n";

const RS_PATTERNS: &str = "use crate::Foo;

pub struct Outer {
    pub inner: Foo,
}

pub fn nested(o: Option<Foo>) -> i32 {
    if let Some(Foo { a, .. }) = o {
        return a;
    }
    0
}

pub fn nested_match(o: Option<Foo>) -> i32 {
    match o {
        Some(Foo { a, .. }) => a,
        None => 0,
    }
}

pub fn tuple_pat(p: (Foo, i32)) -> i32 {
    let (Foo { a, .. }, y) = p;
    a + y
}

pub fn closure() -> i32 {
    let f = |Foo { a, b }: Foo| a + b;
    f(Foo { a: 1, b: 2 })
}

pub fn matches_it(f: &Foo) -> bool {
    matches!(f, Foo { a: 1, .. })
}

pub fn wrapped() -> Outer {
    Outer { inner: Foo { a: 1, b: 2 } }
}

pub fn ref_pat(f: &Foo) -> i32 {
    match f {
        &Foo { a, .. } => a,
    }
}

pub fn while_let(mut it: std::vec::IntoIter<Foo>) -> i32 {
    let mut s = 0;
    while let Some(Foo { a, .. }) = it.next() {
        s += a;
    }
    s
}
";

const RS_SHAPES: &str = "use crate::Foo;

pub fn make() -> Foo {
    Foo { a: 1, b: 2 }
}

pub fn make_ref() -> &'static Foo {
    &Foo { a: 1, b: 2 }
}

pub fn sum_it(f: Foo) -> i32 {
    match f {
        Foo { a, .. } => a,
    }
}

pub fn destruct(f: &Foo) -> i32 {
    let Foo { a, b } = f;
    a + b
}

pub fn many() -> Vec<Foo> {
    vec![Foo { a: 1, b: 1 }, Foo { a: 2, b: 2 }]
}

impl Foo {
    pub fn default_one() -> Foo {
        Foo { a: 0, b: 0 }
    }
}
";

#[test]
fn rust_patterns_nested_in_groups_are_not_constructions_and_literals_in_calls_are() {
    let repo = scanned_repo(&[
        ("src/lib.rs", RS_PATTERN_LIB),
        ("src/pats.rs", RS_PATTERNS),
        ("src/shapes.rs", RS_SHAPES),
    ]);

    let foo = report(&repo, "Foo", Some("src/lib.rs"));

    let mut expected = vec![
        at(&repo, "src/pats.rs", "f(Foo { a: 1, b: 2 })"),
        at(&repo, "src/pats.rs", "Outer { inner: Foo { a: 1, b: 2 } }"),
        format!(
            "src/shapes.rs:{}",
            line_of(&repo, "src/shapes.rs", "pub fn make() -> Foo") + 1
        ),
        at(&repo, "src/shapes.rs", "&Foo { a: 1, b: 2 }"),
        at(&repo, "src/shapes.rs", "vec![Foo { a: 1, b: 1 }"),
        at(&repo, "src/shapes.rs", "Foo { a: 0, b: 0 }"),
    ];
    expected.sort();
    let mut built = constructed(&foo);
    built.sort();
    assert_eq!(built, expected, "{}", dump(&foo));
}

const RS_PATTERN_BESIDE_LITERAL: &str = r#"pub struct Foo {
    pub a: i32,
}

pub fn closure(v: Vec<Foo>) -> Vec<Foo> {
    v.into_iter().map(|Foo { a }| Foo { a: a + 1 }).collect()
}

pub fn arm(f: Foo) -> Foo {
    match f {
        Foo { a } => Foo { a: a + 1 },
    }
}

pub fn if_let(f: Foo) -> Foo {
    if let Foo { a } = f { Foo { a: a + 1 } } else { f }
}

pub fn alternation(f: Foo) -> Foo {
    match f {
        Foo { a: 1 } | Foo { a: 3 } => Foo { a: 9 },
        other => other,
    }
}

pub fn pattern_only(f: Foo) -> i32 {
    let Foo { a } = f;
    a
}

pub fn text_beside_a_pattern(f: Foo) -> String {
    let Foo { a } = f; let s = "Foo { a: 1 }";
    let Foo { a } = f; // Foo { a: 2 }
    s
}
"#;

#[test]
fn a_rust_literal_on_the_line_of_a_pattern_of_the_same_type_is_a_construction() {
    let repo = scanned_repo(&[("src/lib.rs", RS_PATTERN_BESIDE_LITERAL)]);

    let foo = report(&repo, "Foo", None);

    assert_eq!(
        constructed(&foo),
        [
            at(&repo, "src/lib.rs", "map(|Foo { a }|"),
            at(&repo, "src/lib.rs", "Foo { a } => Foo { a: a + 1 }"),
            at(&repo, "src/lib.rs", "if let Foo { a } = f { Foo"),
            at(&repo, "src/lib.rs", "Foo { a: 1 } | Foo { a: 3 }"),
        ],
        "{}",
        dump(&foo)
    );
}

#[test]
fn a_go_literal_on_the_line_of_a_parameter_of_the_same_type_is_a_construction() {
    let repo = scanned_repo(&[
        ("go.mod", "module example.com/m\n\ngo 1.21\n"),
        (
            "a.go",
            "package m\n\ntype Foo struct{ A int }\n\nfunc Bump(f Foo) Foo { return Foo{A: f.A + 1} }\n\nfunc Keep(f Foo) Foo { return f }\n",
        ),
    ]);

    let foo = report(&repo, "Foo", None);

    assert_eq!(
        constructed(&foo),
        [at(&repo, "a.go", "func Bump")],
        "{}",
        dump(&foo)
    );
}

#[test]
fn python_java_and_typescript_calls_to_a_class_are_constructions() {
    let python = scanned_repo(&[
        ("pkg/foo.py", "class Foo:\n    pass\n"),
        (
            "pkg/use.py",
            "from pkg.foo import Foo\n\n\ndef make():\n    return Foo()\n\n\ndef pair(x: Foo):\n    return (Foo(), Foo())\n",
        ),
    ]);
    let java = scanned_repo(&[
        ("Foo.java", "public class Foo {\n}\n"),
        (
            "Main.java",
            "public class Main {\n    Foo make() {\n        return new Foo();\n    }\n\n    void take(Foo f) {\n    }\n}\n",
        ),
    ]);
    let typescript = scanned_repo(&[
        ("src/foo.ts", "export class Foo {}\n"),
        (
            "src/use.ts",
            "import { Foo } from './foo';\n\nexport function make() {\n  return new Foo();\n}\n\nexport function take(f: Foo) {}\n",
        ),
    ]);

    assert_eq!(
        constructed(&report(&python, "Foo", None)),
        ["pkg/use.py:5", "pkg/use.py:9"]
    );
    assert_eq!(constructed(&report(&java, "Foo", None)), ["Main.java:3"]);
    assert_eq!(
        constructed(&report(&typescript, "Foo", None)),
        ["src/use.ts:4"]
    );
}

const GO_STRUCTS: &str = "package m

type Foo struct {
	A int
}

type Box[T any] struct {
	V T
}

func (f Foo) Get() int {
	return f.A
}

func New() Foo {
	return Foo{A: 1}
}

func NewPtr() *Foo {
	return &Foo{A: 2}
}

func Many() []Foo {
	return []Foo{{A: 3}}
}

func Pair() (Foo, error) {
	return Foo{A: 4}, nil
}

func Gen() Box[int] {
	return Box[int]{V: 5}
}

func lit() {
	var y Foo
	_ = y
	f := func() Foo {
		return Foo{A: 6}
	}
	_ = f
}
";

#[test]
fn go_composite_literals_are_constructions_and_return_types_are_not() {
    let repo = scanned_repo(&[
        ("go.mod", "module example.com/m\n\ngo 1.21\n"),
        ("a.go", GO_STRUCTS),
    ]);

    let foo = report(&repo, "Foo", None);
    let boxed = report(&repo, "Box", None);

    assert_eq!(
        constructed(&foo),
        [
            at(&repo, "a.go", "Foo{A: 1}"),
            at(&repo, "a.go", "&Foo{A: 2}"),
            at(&repo, "a.go", "[]Foo{{A: 3}}"),
            at(&repo, "a.go", "Foo{A: 4}"),
            at(&repo, "a.go", "Foo{A: 6}"),
        ],
        "{}",
        dump(&foo)
    );
    assert_eq!(
        constructed(&boxed),
        [at(&repo, "a.go", "Box[int]{V: 5}")],
        "{}",
        dump(&boxed)
    );
}

const GO_LITERALS_AMONG_TEXT: &str = "package main

type Foo struct{ A int }

func raw() Foo { s := `http://x`; _ = s; return Foo{A: 1} }

func plain(f Foo) int { s := \"http://x\"; _ = s; return f.A }

func rune_quote(f Foo) Foo { r := '\"'; _ = r; return Foo{A: 3} }

func in_raw(f Foo) int { s := `Foo{A: 1} \"`; return len(s) + f.A }
";

#[test]
fn a_go_raw_string_or_rune_literal_does_not_hide_a_later_composite_literal() {
    let repo = scanned_repo(&[
        ("go.mod", "module example.com/m\n\ngo 1.21\n"),
        ("a.go", GO_LITERALS_AMONG_TEXT),
    ]);

    let foo = report(&repo, "Foo", None);

    assert_eq!(
        constructed(&foo),
        [
            at(&repo, "a.go", "func raw()"),
            at(&repo, "a.go", "func rune_quote"),
        ],
        "{}",
        dump(&foo)
    );
}

const RS_LITERALS_AMONG_TEXT: &str = r####"pub struct Foo {
    pub a: i32,
}

pub fn in_comment(x: Foo) -> i32 { let Foo { a } = x; /* Foo { a: 1 } */ a }

pub fn after_char(x: Foo, c: char) -> Foo { let Foo { a } = x; if c == '"' { Foo { a } } else { Foo { a: 0 } } }

pub fn in_raw(x: Foo) -> i32 { let Foo { a } = x; let s = r#"say "Foo { a: 1 }" ok"#; a + s.len() as i32 }

pub fn after_comment(x: Foo) -> Foo { let Foo { a } = x; /* done */ Foo { a } }
"####;

#[test]
fn a_rust_comment_or_raw_string_holds_no_construction_and_a_char_quote_hides_none() {
    let repo = scanned_repo(&[("src/lib.rs", RS_LITERALS_AMONG_TEXT)]);

    let foo = report(&repo, "Foo", None);

    assert_eq!(
        constructed(&foo),
        [
            at(&repo, "src/lib.rs", "pub fn after_char"),
            at(&repo, "src/lib.rs", "pub fn after_comment"),
        ],
        "{}",
        dump(&foo)
    );
}

#[test]
fn a_call_that_the_report_excludes_is_not_a_construction() {
    let repo = scanned_repo(&[
        ("a/foo.py", "class Foo:\n    pass\n"),
        (
            "a/use.py",
            "from a.foo import Foo\n\n\ndef make():\n    return Foo()\n",
        ),
        (
            "b/other.py",
            "class Foo:\n    pass\n\n\ndef make():\n    return Foo()\n",
        ),
    ]);

    let far = first_definition_report(&repo, "Foo", "a/foo.py");

    assert_eq!(constructed(&far), ["a/use.py:5"], "{}", dump(&far));
    assert_site(
        &far,
        "b/other.py",
        6,
        SiteLabel::Excluded,
        "resolves to `Foo` (b/other.py:1)",
    );
}

#[test]
fn a_zig_return_type_at_the_start_of_a_line_is_not_a_construction() {
    let repo = scanned_repo(&[(
        "src/point.zig",
        "pub const Point = struct {\n    x: i32,\n};\n\nfn make() Point {\n    return Point{ .x = 1 };\n}\n\npub fn origin() Point {\n    return Point{ .x = 0 };\n}\n",
    )]);

    let point = report(&repo, "Point", None);

    assert_eq!(
        constructed(&point),
        [
            at(&repo, "src/point.zig", "Point{ .x = 1 }"),
            at(&repo, "src/point.zig", "Point{ .x = 0 }"),
        ],
        "{}",
        dump(&point)
    );
}

#[test]
fn a_ruby_comment_that_calls_new_is_not_a_construction() {
    let repo = scanned_repo(&[
        (
            "point.rb",
            "class Point\n  def initialize(x)\n    @x = x\n  end\nend\n",
        ),
        (
            "use.rb",
            "def make\n  Point.new(1)\nend\n\ndef kind\n  Point # Point.new(2)\nend\n",
        ),
    ]);

    let point = report(&repo, "Point", Some("point.rb"));

    assert_eq!(
        constructed(&point),
        [at(&repo, "use.rb", "Point.new(1)")],
        "{}",
        dump(&point)
    );
}

#[test]
fn constructions_print_one_line_with_the_lines_folded_per_file() {
    let repo = scanned_repo(&[
        ("pkg/models.py", "class Foo:\n    pass\n"),
        (
            "pkg/a.py",
            "from pkg.models import Foo\n\n\ndef one():\n    return Foo()\n\n\ndef two():\n    return Foo()\n",
        ),
        (
            "pkg/b.py",
            "from pkg.models import Foo\n\n\ndef three():\n    return Foo()\n",
        ),
        ("notes.md", "Foo is documented here\n"),
    ]);
    let foo = report(&repo, "Foo", None);

    let text = format_reference_report(&foo, 20);
    let folded = format_reference_report(&foo, 2);

    assert!(
        text.contains("Constructions (3): pkg/a.py: 5, 9 · pkg/b.py: 5\nText only (1): "),
        "{text}"
    );
    assert!(
        folded.contains(
            "Constructions (3): pkg/a.py: 5, 9\n… 1 more construction in 1 file: pkg/b.py (1)\n"
        ),
        "{folded}"
    );
    assert_eq!(
        serde_json::to_value(&foo).unwrap()["constructions"],
        serde_json::json!([
            {"path": "pkg/a.py", "line": 5},
            {"path": "pkg/a.py", "line": 9},
            {"path": "pkg/b.py", "line": 5},
        ])
    );
}

#[test]
fn a_type_with_no_construction_and_a_function_print_no_construction_section() {
    let repo = scanned_repo(&[
        (
            "pkg/models.py",
            "class Foo:\n    pass\n\n\ndef helper():\n    return 1\n",
        ),
        (
            "pkg/use.py",
            "from pkg.models import helper\n\n\ndef go():\n    return helper()\n",
        ),
    ]);

    for name in ["Foo", "helper"] {
        let report = report(&repo, name, None);
        assert!(report.constructions.is_empty());
        assert!(!format_reference_report(&report, 20).contains("Constructions"));
        assert!(
            serde_json::to_value(&report)
                .unwrap()
                .get("constructions")
                .is_none()
        );
    }
}

const RS_ENUM: &str = "pub enum Kind {
    A,
    B,
    C(i32),
}

pub fn classify(k: Kind) -> i32 {
    match k {
        Kind::A => 1,
        Kind::B => 2,
        Kind::C(n) => n,
    }
}

pub fn single() -> Kind {
    Kind::B
}
";

#[test]
fn a_rust_enum_lists_the_functions_that_name_its_variants_most_variants_first() {
    let repo = scanned_repo(&[("src/lib.rs", RS_ENUM)]);

    let kind = report(&repo, "Kind", None);

    let rows: Vec<(String, usize, usize, usize)> = kind
        .variant_uses
        .iter()
        .map(|row| {
            (
                row.caller.clone(),
                row.line,
                row.variants_used,
                row.variants_total,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                "classify".to_string(),
                line_of(&repo, "src/lib.rs", "Kind::A"),
                3,
                3
            ),
            (
                "single".to_string(),
                line_of(&repo, "src/lib.rs", "fn single") + 1,
                1,
                3
            ),
        ],
        "{}",
        dump(&kind)
    );
    assert!(kind.constructions.is_empty());
}

#[test]
fn variant_uses_print_after_the_sites_with_the_function_count_and_fold_after_the_limit() {
    let repo = scanned_repo(&[
        ("src/lib.rs", RS_ENUM),
        ("notes.md", "Kind is documented here\n"),
    ]);
    let kind = report(&repo, "Kind", None);
    let first = line_of(&repo, "src/lib.rs", "Kind::A");
    let second = line_of(&repo, "src/lib.rs", "fn single") + 1;

    let text = format_reference_report(&kind, 20);
    let folded = format_reference_report(&kind, 1);

    assert!(
        text.contains(&format!(
            "Variant uses (2 functions): a match in them may need a new arm\n\
             - `classify` [src/lib.rs:{first}] names 3 of 3 variants\n\
             - `single` [src/lib.rs:{second}] names 1 of 3 variants\n\
             Text only (1): "
        )),
        "{text}"
    );
    assert!(
        folded.contains(&format!(
            "- `classify` [src/lib.rs:{first}] names 3 of 3 variants\n\
             … 1 more function in 1 file: src/lib.rs (1)\n"
        )),
        "{folded}"
    );
    let json = serde_json::to_value(&kind).unwrap();
    assert_eq!(json["variant_uses"][0]["variants_used"], 3);
    assert_eq!(json["variant_uses"][0]["variants_total"], 3);
    assert_eq!(json["variant_uses"][0]["caller"], "classify");
}

#[test]
fn variant_rows_with_the_same_count_sort_by_path() {
    let repo = scanned_repo(&[
        (
            "src/lib.rs",
            "pub mod b;\npub mod a;\n\npub enum Kind {\n    A,\n    B,\n}\n",
        ),
        (
            "src/b.rs",
            "use crate::Kind;\n\npub fn from_b() -> Kind {\n    Kind::A\n}\n",
        ),
        (
            "src/a.rs",
            "use crate::Kind;\n\npub fn from_a() -> Kind {\n    Kind::B\n}\n",
        ),
    ]);

    let kind = report(&repo, "Kind", None);

    let rows: Vec<(&str, &str)> = kind
        .variant_uses
        .iter()
        .map(|row| (row.path.as_str(), row.caller.as_str()))
        .collect();
    assert_eq!(
        rows,
        [("src/a.rs", "from_a"), ("src/b.rs", "from_b")],
        "{}",
        dump(&kind)
    );
}

#[test]
fn qualified_member_accesses_of_python_typescript_csharp_and_java_enums_are_variant_uses() {
    let python = scanned_repo(&[(
        "color.py",
        "from enum import Enum\n\n\nclass Color(Enum):\n    RED = 1\n    GREEN = 2\n    BLUE = 3\n\n\nRED_ALERT = 4\n\n\ndef pick(c):\n    if c == Color.RED:\n        return 1\n    elif c == Color.GREEN:\n        return 2\n    return Color.BLUE\n\n\ndef unrelated():\n    RED = 5\n    return RED\n",
    )]);
    let typescript = scanned_repo(&[(
        "color.ts",
        "export enum Shade {\n  Light,\n  Dark,\n}\n\nexport function flip(s: Shade): Shade {\n  return s === Shade.Light ? Shade.Dark : Shade.Light;\n}\n",
    )]);
    let csharp = scanned_repo(&[(
        "Mode.cs",
        "public enum Mode { Fast, Slow }\n\npublic class Runner {\n    public int Run(Mode m) {\n        return m == Mode.Fast ? 1 : 2;\n    }\n}\n",
    )]);
    let java = scanned_repo(&[(
        "Level.java",
        "public enum Level {\n    LOW,\n    HIGH;\n\n    static int score(Level l) {\n        switch (l) {\n            case LOW:\n                return 1;\n            default:\n                return Level.HIGH.ordinal();\n        }\n    }\n}\n",
    )]);

    let rows = |repo: &Repo, name: &str| -> Vec<String> {
        report(repo, name, None)
            .variant_uses
            .iter()
            .map(|row| {
                format!(
                    "{} {} of {}",
                    row.caller, row.variants_used, row.variants_total
                )
            })
            .collect()
    };

    assert_eq!(rows(&python, "Color"), ["pick 3 of 3"]);
    assert_eq!(rows(&typescript, "Shade"), ["flip 2 of 2"]);
    assert_eq!(rows(&csharp, "Mode"), ["Run 1 of 2"]);
    assert_eq!(rows(&java, "Level"), ["score 2 of 2"]);
}

#[test]
fn a_name_equal_to_a_variant_on_a_line_without_the_enum_is_not_a_variant_use() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub enum Kind {\n    A,\n    B,\n}\n\npub fn shadow() -> i32 {\n    let A = 1;\n    A\n}\n",
    )]);

    let kind = report(&repo, "Kind", None);

    assert!(kind.variant_uses.is_empty(), "{:?}", kind.variant_uses);
    let text = format_reference_report(&kind, 20);
    assert!(!text.contains("Variant uses"), "{text}");
    assert!(
        serde_json::to_value(&kind)
            .unwrap()
            .get("variant_uses")
            .is_none()
    );
}

const RS_STATUS: &str = "pub enum Status {
    Ok,
    Err,
    Some,
    Warn,
}

pub fn parse(n: i32, s: Status) -> Option<Result<i32, ()>> {
    let w = Status::Warn;
    if matches!(s, Status::Warn) { Some(Ok(n)) } else { Some(Err(())) }
}

pub fn qualified(s: Status) -> Status {
    match s {
        Status::Ok => Status::Err,
        _ => crate::Status::Some,
    }
}
";

#[test]
fn rust_names_that_equal_variants_count_only_when_the_enum_qualifies_them() {
    let repo = scanned_repo(&[("src/lib.rs", RS_STATUS)]);

    let status = report(&repo, "Status", None);

    let rows: Vec<String> = status
        .variant_uses
        .iter()
        .map(|row| {
            format!(
                "{} {} of {}",
                row.caller, row.variants_used, row.variants_total
            )
        })
        .collect();
    assert_eq!(
        rows,
        ["qualified 3 of 4", "parse 1 of 4"],
        "{}",
        dump(&status)
    );
}

#[test]
fn a_typescript_call_that_shares_a_variant_name_is_not_a_variant_use_but_the_member_access_is() {
    let repo = scanned_repo(&[(
        "color.ts",
        "export enum Shade {\n  Light,\n  Dark,\n}\n\nexport function Dark(): void {}\n\nexport function flip(s: Shade): Shade {\n  return s === Shade.Light ? Dark() : s;\n}\n",
    )]);

    let shade = report(&repo, "Shade", Some("color.ts"));

    let rows: Vec<String> = shade
        .variant_uses
        .iter()
        .map(|row| {
            format!(
                "{} {} of {}",
                row.caller, row.variants_used, row.variants_total
            )
        })
        .collect();
    assert_eq!(rows, ["flip 1 of 2"], "{}", dump(&shade));
}

fn variant_rows(repo: &Repo, name: &str) -> Vec<String> {
    report(repo, name, None)
        .variant_uses
        .iter()
        .map(|row| {
            format!(
                "{} {} of {}",
                row.caller, row.variants_used, row.variants_total
            )
        })
        .collect()
}

#[test]
fn an_unscoped_c_enum_counts_a_bare_enumerator_on_a_line_with_the_enum() {
    let repo = scanned_repo(&[(
        "color.c",
        "enum Color { RED, GREEN, BLUE };\n\nint rank(enum Color c) { return c == RED ? 1 : 0; }\n\nint mix(void) {\n    enum Color a = RED, b = GREEN;\n    return a == b;\n}\n\nint plain(void) {\n    int x = GREEN;\n    return x;\n}\n",
    )]);

    assert_eq!(
        variant_rows(&repo, "Color"),
        ["mix 2 of 3", "rank 1 of 3"],
        "{}",
        dump(&report(&repo, "Color", None))
    );
}

#[test]
fn an_unscoped_c_enum_counts_a_bare_enumerator_in_a_function_that_names_the_enum() {
    let c = scanned_repo(&[(
        "color.c",
        "enum Color { RED, GREEN, BLUE };\n\nint rank(enum Color c) {\n    switch (c) {\n    case RED: return 1;\n    case GREEN: return 2;\n    default: return 0;\n    }\n}\n\nint shadow(enum Color c) {\n    int GREEN = 3;\n    return c + GREEN;\n}\n\nint plain(int c) {\n    switch (c) {\n    case RED: return 1;\n    default: return 0;\n    }\n}\n",
    )]);
    let cpp = scanned_repo(&[(
        "shade.cpp",
        "enum Shade { Light, Dark, Dim };\n\nenum class Mode { Fast, Slow };\n\nint pick(Shade s, Mode m) {\n    switch (s) {\n    case Light: return 1;\n    case Dim: return 2;\n    default: break;\n    }\n    switch (m) {\n    case Mode::Fast: return 3;\n    default: return 0;\n    }\n}\n",
    )]);

    assert_eq!(
        variant_rows(&c, "Color"),
        ["rank 2 of 3"],
        "{}",
        dump(&report(&c, "Color", None))
    );
    assert_eq!(
        variant_rows(&cpp, "Shade"),
        ["pick 2 of 3"],
        "{}",
        dump(&report(&cpp, "Shade", None))
    );
    assert_eq!(
        variant_rows(&cpp, "Mode"),
        ["pick 1 of 2"],
        "{}",
        dump(&report(&cpp, "Mode", None))
    );
}

#[test]
fn a_parameter_or_local_named_like_an_enumerator_is_not_a_variant_use_on_a_site_line() {
    let c = scanned_repo(&[(
        "color.c",
        "enum Color { RED, GREEN, BLUE };\n\nint named(enum Color c, int BLUE) { return BLUE + c; }\n\nint pointer(enum Color c, int *BLUE) { return *BLUE + c; }\n\nint split(enum Color c,\n          int BLUE) {\n    return BLUE + c;\n}\n\nint old(BLUE, c) enum Color c; int BLUE; { return BLUE + c; }\n\nint local(enum Color c) { int GREEN = 2; return c + GREEN; }\n\nint real(enum Color c) { return c == BLUE; }\n",
    )]);
    let cpp = scanned_repo(&[(
        "color.cpp",
        "enum Color { RED, GREEN, BLUE };\n\nint named(Color c, int BLUE = 3) { return BLUE + c; }\n\nint fallback(Color c, int x = BLUE) { return x + c; }\n",
    )]);

    assert_eq!(
        variant_rows(&c, "Color"),
        ["real 1 of 3"],
        "{}",
        dump(&report(&c, "Color", None))
    );
    assert_eq!(
        variant_rows(&cpp, "Color"),
        ["fallback 1 of 3"],
        "{}",
        dump(&report(&cpp, "Color", None))
    );
}

#[test]
fn an_unscoped_cpp_enum_counts_bare_and_qualified_enumerators() {
    let repo = scanned_repo(&[(
        "color.cpp",
        "enum Color { RED, GREEN, BLUE };\n\nColor paint(Color c) {\n    Color d = RED;\n    return c == Color::GREEN ? d : c;\n}\n\nint plain() {\n    int x = BLUE;\n    return x;\n}\n",
    )]);

    assert_eq!(
        variant_rows(&repo, "Color"),
        ["paint 2 of 3"],
        "{}",
        dump(&report(&repo, "Color", None))
    );
}

#[test]
fn a_cpp_scoped_enum_counts_only_the_qualified_enumerator() {
    let class = scanned_repo(&[(
        "shade.cpp",
        "enum class Shade { Light, Dark };\n\nint Light = 1;\n\nShade bare() {\n    Shade s = static_cast<Shade>(Light);\n    return s;\n}\n\nShade qualified() {\n    Shade s = Shade::Dark;\n    return s;\n}\n",
    )]);
    let record = scanned_repo(&[(
        "mode.cpp",
        "enum struct Mode { Fast, Slow };\n\nint Slow = 2;\n\nMode pick() {\n    Mode m = static_cast<Mode>(Slow);\n    return m;\n}\n",
    )]);

    assert_eq!(
        variant_rows(&class, "Shade"),
        ["qualified 1 of 2"],
        "{}",
        dump(&report(&class, "Shade", None))
    );
    assert!(variant_rows(&record, "Mode").is_empty());
}

const RS_SELF: &str = "pub enum Kind {
    Circle,
    Square,
    Triangle,
}

impl Kind {
    pub fn label(&self) -> i32 {
        match self {
            Self::Circle => 1,
            Self::Square => 2,
            Self::Triangle => 3,
        }
    }

    pub fn closure(&self) -> i32 {
        let f = |x: i32| match self {
            Self::Circle => x,
            _ => 0,
        };
        f(1)
    }
}

impl std::fmt::Display for Kind {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Self::Square => write!(f, \"s\"),
            _ => Ok(()),
        }
    }
}

pub struct Other;

impl Other {
    const Circle: i32 = 1;

    pub fn circle() -> i32 {
        Self::Circle
    }
}
";

#[test]
fn a_rust_self_variant_in_an_impl_of_the_enum_is_a_variant_use() {
    let repo = scanned_repo(&[("src/lib.rs", RS_SELF)]);

    assert_eq!(
        variant_rows(&repo, "Kind"),
        ["label 3 of 3", "closure 1 of 3", "fmt 1 of 3"],
        "{}",
        dump(&report(&repo, "Kind", None))
    );
}

const RS_GLOB: &str = "pub enum Kind {
    Circle,
    Square,
    Triangle,
}

use Kind::*;

pub fn glob(k: Kind) -> i32 {
    match k {
        Circle => 1,
        Square => 2,
        Triangle => 3,
    }
}

pub fn no_site() -> i32 {
    Circle as i32
}

pub fn texts(k: Kind) -> &'static str {
    let _ = k;
    // Circle
    \"Square\"
}

mod named {
    use super::Kind;
    use super::Kind::{Circle, Square};

    pub fn pick(k: Kind) -> i32 {
        match k {
            Circle => 1,
            Square => 2,
            _ => 3,
        }
    }

    pub fn other_module(k: Kind) -> i32 {
        match k {
            Triangle => 1,
            _ => 0,
        }
    }
}
";

#[test]
fn a_bare_rust_variant_counts_where_the_scope_imports_it_in_a_function_that_holds_a_site() {
    let repo = scanned_repo(&[("src/lib.rs", RS_GLOB)]);

    assert_eq!(
        variant_rows(&repo, "Kind"),
        ["glob 3 of 3", "pick 2 of 3"],
        "{}",
        dump(&report(&repo, "Kind", None))
    );
}

const RS_LOCAL_GLOB: &str = "pub enum Kind {
    Circle,
    Square,
}

impl Kind {
    pub fn within(&self) -> i32 {
        use Kind::*;
        match self {
            Circle => 1,
            Square => 2,
        }
    }
}

pub fn local_use(k: Kind) -> i32 {
    use Kind::*;
    match k {
        Circle => 1,
        Square => 2,
    }
}

pub fn unimported(k: Kind) -> i32 {
    match k {
        Circle => 1,
        _ => 0,
    }
}
";

#[test]
fn a_rust_import_inside_a_function_or_method_scopes_the_bare_variants_to_it() {
    let repo = scanned_repo(&[("src/lib.rs", RS_LOCAL_GLOB)]);

    assert_eq!(
        variant_rows(&repo, "Kind"),
        ["within 2 of 2", "local_use 2 of 2"],
        "{}",
        dump(&report(&repo, "Kind", None))
    );
}

#[test]
fn a_bare_rust_variant_with_no_import_in_scope_is_not_a_variant_use() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub enum Kind {\n    Circle,\n    Square,\n}\n\npub fn shadow(k: Kind) -> i32 {\n    let Circle = 1;\n    Circle + k as i32\n}\n",
    )]);

    assert!(
        variant_rows(&repo, "Kind").is_empty(),
        "{}",
        dump(&report(&repo, "Kind", None))
    );
}

const JAVA_CASES: &[(&str, &str)] = &[
    (
        "src/Kind.java",
        "package p;\n\npublic enum Kind {\n    CIRCLE,\n    SQUARE,\n    TRIANGLE\n}\n",
    ),
    (
        "src/Use.java",
        "package p;\n\npublic class Use {\n    static final int CIRCLE = 1;\n    static final int SQUARE = 2;\n\n    static int multi(Kind k) {\n        switch (k) {\n            case CIRCLE, SQUARE:\n                return 1;\n            default:\n                return 0;\n        }\n    }\n\n    static int arrow(Kind k) {\n        return switch (k) {\n            case CIRCLE, SQUARE -> 1;\n            case TRIANGLE -> 2;\n        };\n    }\n\n    static int no_site(int x) {\n        switch (x) {\n            case CIRCLE:\n                return 1;\n            default:\n                return 0;\n        }\n    }\n\n    static int not_after_case(Kind k) {\n        int y = SQUARE;\n        return y + k.ordinal();\n    }\n}\n",
    ),
];

#[test]
fn a_java_case_label_counts_a_variant_in_a_function_that_holds_a_site_of_the_enum() {
    let repo = scanned_repo(JAVA_CASES);

    assert_eq!(
        variant_rows(&repo, "Kind"),
        ["arrow 3 of 3", "multi 2 of 3"],
        "{}",
        dump(&report(&repo, "Kind", None))
    );
}

const FLASK_VIEW: (&str, &str) = (
    "app.py",
    "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/x\", methods=[\"POST\"])\ndef target():\n    return \"x\"\n\n\napp.add_url_rule(\"/y\", view_func=target)\n",
);

fn registered(report: &ReferenceReport) -> Vec<String> {
    report
        .registrations
        .iter()
        .map(|row| {
            format!(
                "{} {}:{}{}",
                row.pattern_id,
                row.path,
                row.line,
                if row.by_name { " (by name)" } else { "" }
            )
        })
        .collect()
}

#[test]
fn a_flask_view_lists_its_decorator_route_and_its_add_url_rule_by_name() {
    let repo = scanned_repo(&[FLASK_VIEW]);
    let decorator = line_of(&repo, "app.py", "@app.route");
    let rule = line_of(&repo, "app.py", "add_url_rule");

    let target = report(&repo, "target", None);

    assert_eq!(
        registered(&target),
        [
            format!("flask.route.v1 app.py:{decorator}"),
            format!("flask.route.v1 app.py:{rule} (by name)"),
        ],
        "{}",
        dump(&target)
    );
    let text = format_reference_report(&target, 20);
    assert!(
        text.contains(&format!(
            "Registrations (2):\n\
             - route POST /x [app.py:{decorator}]\n\
             - route GET /y [app.py:{rule}] (by name)\n"
        )),
        "{text}"
    );
}

#[test]
fn a_function_of_the_same_name_in_another_file_gets_no_row_from_a_file_that_defines_its_own() {
    let repo = scanned_repo(&[
        FLASK_VIEW,
        ("other/views.py", "def target():\n    return \"other\"\n"),
    ]);

    let other = report(&repo, "target", Some("other/views.py"));

    assert!(other.registrations.is_empty(), "{}", dump(&other));
}

const FLASK_IMPORTING_REGISTRAR: (&str, &str) = (
    "other/reg.py",
    "from flask import Flask\nfrom .views import target\n\napp2 = Flask(__name__)\napp2.add_url_rule(\"/other\", view_func=target)\n",
);

#[test]
fn a_view_registered_through_an_import_keeps_its_by_name_row() {
    let repo = scanned_repo(&[
        FLASK_VIEW,
        ("other/views.py", "def target():\n    return \"other\"\n"),
        FLASK_IMPORTING_REGISTRAR,
    ]);
    let registrar = line_of(&repo, "other/reg.py", "add_url_rule");

    let other = report(&repo, "target", Some("other/views.py"));

    assert_eq!(
        registered(&other),
        [format!("flask.route.v1 other/reg.py:{registrar} (by name)")],
        "{}",
        dump(&other)
    );
}

#[test]
fn a_by_name_row_at_a_site_the_report_labels_excluded_is_dropped() {
    let repo = scanned_repo(&[
        FLASK_VIEW,
        ("other/views.py", "def target():\n    return \"other\"\n"),
        FLASK_IMPORTING_REGISTRAR,
    ]);
    let decorator = line_of(&repo, "app.py", "@app.route");
    let rule = line_of(&repo, "app.py", "add_url_rule");
    let registrar = line_of(&repo, "other/reg.py", "add_url_rule");

    let target = report(&repo, "target", Some("app.py"));

    assert_site(
        &target,
        "other/reg.py",
        registrar,
        SiteLabel::Excluded,
        "resolves to `target` (other/views.py:1)",
    );
    assert_eq!(
        registered(&target),
        [
            format!("flask.route.v1 app.py:{decorator}"),
            format!("flask.route.v1 app.py:{rule} (by name)"),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_by_name_row_is_dropped_when_the_excluded_site_is_on_a_later_line_of_the_fact() {
    let repo = scanned_repo(&[
        FLASK_VIEW,
        ("other/views.py", "def target():\n    return \"other\"\n"),
        (
            "other/reg.py",
            "from flask import Flask\nfrom .views import target\n\napp2 = Flask(__name__)\napp2.add_url_rule(\n    \"/other\",\n    view_func=target,\n)\n",
        ),
    ]);
    let decorator = line_of(&repo, "app.py", "@app.route");
    let rule = line_of(&repo, "app.py", "add_url_rule");
    let call = line_of(&repo, "other/reg.py", "add_url_rule");
    let view = line_of(&repo, "other/reg.py", "view_func");

    let target = report(&repo, "target", Some("app.py"));

    assert!(call < view);
    assert_site(
        &target,
        "other/reg.py",
        view,
        SiteLabel::Excluded,
        "resolves to `target` (other/views.py:1)",
    );
    assert_eq!(
        registered(&target),
        [
            format!("flask.route.v1 app.py:{decorator}"),
            format!("flask.route.v1 app.py:{rule} (by name)"),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_local_function_that_a_test_registers_is_not_a_registration_of_the_routed_view() {
    let repo = scanned_repo(&[
        (
            "views.py",
            "from flask import Blueprint\n\nbp = Blueprint(\"bp\", __name__)\n\n\n@bp.route(\"/\")\ndef index():\n    return \"real\"\n",
        ),
        (
            "tests/test_basic.py",
            "from flask import Flask\n\n\ndef test_local():\n    app = Flask(__name__)\n\n    def index():\n        return \"local\"\n\n    app.add_url_rule(\"/\", view_func=index)\n",
        ),
    ]);
    let decorator = line_of(&repo, "views.py", "@bp.route");
    let rule = line_of(&repo, "tests/test_basic.py", "add_url_rule");

    let routed = report(&repo, "index", Some("views.py"));
    let local = report(&repo, "index", Some("tests/test_basic.py"));

    assert_eq!(
        registered(&routed),
        [format!("flask.route.v1 views.py:{decorator}")],
        "{}",
        dump(&routed)
    );
    assert_eq!(
        registered(&local),
        [format!(
            "flask.route.v1 tests/test_basic.py:{rule} (by name)"
        )],
        "{}",
        dump(&local)
    );
}

const FLASK_HANDLER: (&str, &str) = ("handlers.py", "def list_users():\n    return []\n");

const FLASK_ROUTE_WITH_METHOD: (&str, &str) = (
    "routes.py",
    "from flask import Flask\nfrom handlers import list_users\n\napp = Flask(__name__)\n\n\nclass Service:\n    def list_users(self):\n        return [\"svc\"]\n\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
);

#[test]
fn a_method_of_the_same_name_in_the_fact_file_does_not_drop_an_imported_registration() {
    let repo = scanned_repo(&[FLASK_HANDLER, FLASK_ROUTE_WITH_METHOD]);
    let rule = line_of(&repo, "routes.py", "add_url_rule");

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert_site(&users, "routes.py", rule, SiteLabel::Proven, "import");
    assert_eq!(
        registered(&users),
        [format!("flask.route.v1 routes.py:{rule} (by name)")],
        "{}",
        dump(&users)
    );
}

#[test]
fn a_method_of_the_same_name_does_not_drop_a_registration_the_report_cannot_tie() {
    let repo = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\nclass Service:\n    def list_users(self):\n        return [\"svc\"]\n\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);
    let rule = line_of(&repo, "routes.py", "add_url_rule");

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert_eq!(
        site_at(&users, "routes.py", rule).label,
        SiteLabel::Unresolved,
        "{}",
        dump(&users)
    );
    assert_eq!(
        registered(&users),
        [format!("flask.route.v1 routes.py:{rule} (by name)")],
        "{}",
        dump(&users)
    );
}

#[test]
fn a_class_attribute_of_the_same_name_does_not_drop_an_unresolved_registration() {
    let repo = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\nclass Service:\n    list_users = None\n\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);
    let rule = line_of(&repo, "routes.py", "add_url_rule");

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert_eq!(
        registered(&users),
        [format!("flask.route.v1 routes.py:{rule} (by name)")],
        "{}",
        dump(&users)
    );
}

#[test]
fn a_parameter_of_the_same_name_in_another_function_does_not_drop_an_unresolved_registration() {
    let repo = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\napp.add_url_rule(\"/users\", view_func=list_users)\n\n\ndef helper(list_users=None):\n    return list_users\n",
        ),
    ]);
    let rule = line_of(&repo, "routes.py", "add_url_rule");

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert_eq!(
        site_at(&users, "routes.py", rule).label,
        SiteLabel::Unresolved,
        "{}",
        dump(&users)
    );
    assert_eq!(
        registered(&users),
        [format!("flask.route.v1 routes.py:{rule} (by name)")],
        "{}",
        dump(&users)
    );
}

#[test]
fn a_parameter_of_the_function_that_holds_the_fact_drops_an_unresolved_registration() {
    let repo = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\ndef register(list_users=None):\n    app.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert!(users.registrations.is_empty(), "{}", dump(&users));
}

const FLASK_SELF_HANDLER: (&str, &str) = (
    "tests/routes.py",
    "from flask import Flask\n\napp = Flask(__name__)\n\n\ndef test_rules():\n    def index():\n        return \"local\"\n\n    app.add_url_rule(\"/\", view_func=index)\n\n\ndef test_class():\n    class View:\n        def __init__(self, app):\n            app.add_url_rule(\"/v\", \"index\", self.index)\n\n        def index(self):\n            return \"member\"\n\n    View(app)\n",
);

fn repo_with_self_handler() -> Repo {
    let alternatives: Vec<(String, String)> = (0..14)
        .map(|n| {
            (
                format!("app/alt{n}.py"),
                "def index():\n    return \"alt\"\n".to_string(),
            )
        })
        .collect();
    let mut files: Vec<(&str, &str)> = alternatives
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    files.push(("app/views.py", "def index():\n    return \"real\"\n"));
    files.push(FLASK_SELF_HANDLER);
    scanned_repo(&files)
}

#[test]
fn a_self_qualified_handler_does_not_register_a_function_of_the_same_name() {
    let repo = repo_with_self_handler();

    let function = report(&repo, "index", Some("app/views.py"));

    assert!(function.registrations.is_empty(), "{}", dump(&function));
}

#[test]
fn a_self_qualified_handler_registers_the_method_of_the_same_name_by_name() {
    let repo = repo_with_self_handler();
    let rule = line_of(&repo, "tests/routes.py", "self.index");
    let method = line_of(&repo, "tests/routes.py", "def index(self)");

    let member = report_at(&repo, "index", "tests/routes.py", method);

    assert_eq!(
        registered(&member),
        [format!("flask.route.v1 tests/routes.py:{rule} (by name)")],
        "{}",
        dump(&member)
    );
}

#[test]
fn a_module_level_variable_or_class_of_the_same_name_drops_an_unresolved_registration() {
    let variable = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\nlist_users = lambda: []\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);
    let class = scanned_repo(&[
        FLASK_HANDLER,
        ("alt.py", "def list_users():\n    return [\"alt\"]\n"),
        (
            "routes.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\nclass list_users:\n    pass\n\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);

    assert!(
        report(&variable, "list_users", Some("handlers.py"))
            .registrations
            .is_empty()
    );
    assert!(
        report(&class, "list_users", Some("handlers.py"))
            .registrations
            .is_empty()
    );
}

#[test]
fn a_proven_site_on_the_lines_of_the_fact_keeps_the_row_despite_a_function_nested_elsewhere() {
    let repo = scanned_repo(&[
        FLASK_HANDLER,
        (
            "routes.py",
            "from flask import Flask\nfrom handlers import list_users\n\napp = Flask(__name__)\n\n\ndef make_stub():\n    def list_users():\n        return []\n\n    return list_users\n\n\napp.add_url_rule(\"/users\", view_func=list_users)\n",
        ),
    ]);
    let rule = line_of(&repo, "routes.py", "add_url_rule");

    let users = report(&repo, "list_users", Some("handlers.py"));

    assert_site(&users, "routes.py", rule, SiteLabel::Proven, "import");
    assert_eq!(
        registered(&users),
        [format!("flask.route.v1 routes.py:{rule} (by name)")],
        "{}",
        dump(&users)
    );
}

#[test]
fn registrations_print_after_the_sites_and_before_the_text_only_lines() {
    let repo = scanned_repo(&[FLASK_VIEW, ("notes.md", "target is documented here\n")]);
    let decorator = line_of(&repo, "app.py", "@app.route");
    let rule = line_of(&repo, "app.py", "add_url_rule");

    let text = format_reference_report(&report(&repo, "target", None), 20);

    assert!(
        text.contains(&format!(
            "- (no enclosing symbol) [app.py:{rule}] (kind: variable_ref; same file)\n\
             Registrations (2):\n\
             - route POST /x [app.py:{decorator}]\n\
             - route GET /y [app.py:{rule}] (by name)\n\
             Text only (1): "
        )),
        "{text}"
    );
}

#[test]
fn registration_rows_fold_after_the_limit_with_the_full_count() {
    let repo = scanned_repo(&[FLASK_VIEW]);
    let rule = line_of(&repo, "app.py", "add_url_rule");

    let target = report(&repo, "target", None);
    let text = format_reference_report(&target, 1);

    assert_eq!(target.registrations.len(), 2);
    assert!(
        text.contains(&format!(
            "Registrations (2):\n\
             - route POST /x [app.py:{}]\n\
             … 1 more registration in 1 file: app.py (1)\n",
            line_of(&repo, "app.py", "@app.route")
        )),
        "{text}"
    );
    assert!(
        !text.contains(&format!("app.py:{rule}] (by name)")),
        "{text}"
    );
    let json = serde_json::to_value(&target).unwrap();
    assert_eq!(json["registrations"][0]["pattern_id"], "flask.route.v1");
    assert_eq!(json["registrations"][0]["verb"], "POST");
    assert_eq!(json["registrations"][0]["template"], "/x");
    assert_eq!(json["registrations"][0]["by_name"], false);
    assert_eq!(json["registrations"][1]["by_name"], true);
}

#[test]
fn a_function_no_framework_fact_names_prints_no_registration_section() {
    let repo = scanned_repo(&[FLASK_VIEW, ("lib.py", "def helper():\n    return 1\n")]);

    let helper = report(&repo, "helper", None);

    assert!(helper.registrations.is_empty());
    assert!(!format_reference_report(&helper, 20).contains("Registrations"));
    assert!(
        serde_json::to_value(&helper)
            .unwrap()
            .get("registrations")
            .is_none()
    );
}

#[test]
fn a_minimal_api_handler_is_registered_by_name_and_a_fastapi_decorator_by_id() {
    let csharp = scanned_repo(&[(
        "Program.cs",
        "var app = WebApplication.Create();\n\napp.MapGet(\"/hello\", Hello);\n\nstatic string Hello() => \"hi\";\n",
    )]);
    let fastapi = scanned_repo(&[(
        "main.py",
        "from fastapi import FastAPI\n\napp = FastAPI()\n\n\n@app.get(\"/items\")\ndef items():\n    return []\n",
    )]);

    let hello = report(&csharp, "Hello", None);
    let items = report(&fastapi, "items", None);

    assert_eq!(
        registered(&hello),
        [format!(
            "aspnet.minimal_api.route.v1 Program.cs:{} (by name)",
            line_of(&csharp, "Program.cs", "MapGet")
        )],
        "{}",
        dump(&hello)
    );
    assert_eq!(
        registered(&items),
        [format!(
            "fastapi.route.v1 main.py:{}",
            line_of(&fastapi, "main.py", "def items")
        )],
        "{}",
        dump(&items)
    );
    assert!(
        format_reference_report(&items, 20).contains("- route GET /items [main.py:"),
        "{}",
        dump(&items)
    );
}

fn insert_fact(repo: &Repo, pattern_id: &str, containing: Option<&str>, metadata: &str) {
    let conn = rusqlite::Connection::open(&repo.db).unwrap();
    let file_id: String = conn
        .query_row(
            "SELECT file_id FROM files WHERE path = 'app.py'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let serial: i64 = conn
        .query_row("SELECT COUNT(*) FROM structural_facts", [], |row| {
            row.get(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO structural_facts (structural_fact_id, file_id, path, language, pattern_id,
            capture_name, node_kind, containing_symbol_id, start_line, start_column, end_line,
            end_column, start_byte, end_byte, confidence, metadata_json)
         VALUES (?1, ?2, 'app.py', 'python', ?3, 'route', 'call', ?4, ?5, 0, ?5, 1, 0, 1, 1.0, ?6)",
        rusqlite::params![
            format!("fact-{serial}"),
            file_id,
            pattern_id,
            containing,
            100 + serial,
            metadata
        ],
    )
    .unwrap();
}

#[test]
fn every_handler_key_of_a_route_fact_registers_a_function_of_that_name_by_name() {
    let repo = scanned_repo(&[(
        "app.py",
        "def target():\n    return 1\n\n\ndef other():\n    return 2\n",
    )]);
    let target_id: String = open_read_only(&repo.db)
        .unwrap()
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'target'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    for key in [
        "view_target",
        "handler",
        "handler_name",
        "controller_action",
        "view_name",
        "function_name",
    ] {
        insert_fact(
            &repo,
            "demo.route.v1",
            None,
            &format!(r#"{{"{key}":"target","route_template":"/{key}"}}"#),
        );
    }
    insert_fact(
        &repo,
        "demo.router_mount.v1",
        None,
        r#"{"handler_names":["other","target"]}"#,
    );
    insert_fact(&repo, "demo.route.v1", None, r#"{"handler":"other"}"#);
    insert_fact(
        &repo,
        "demo.route.v1",
        None,
        r#"{"handler_names":["other"]}"#,
    );
    insert_fact(&repo, "demo.route.v1", None, r#"{"handler":"Target"}"#);
    insert_fact(&repo, "demo.route.v1", None, "not json");
    insert_fact(
        &repo,
        "sql.table_definition.v1",
        None,
        r#"{"function_name":"target"}"#,
    );
    insert_fact(
        &repo,
        "demo.route.v1",
        Some(&target_id),
        r#"{"api_style":"call_routing","route_template":"/registrar"}"#,
    );
    insert_fact(
        &repo,
        "demo.decorated.v1",
        Some(&target_id),
        r#"{"api_style":"decorator_routing"}"#,
    );

    let target = report(&repo, "target", None);

    let lines: Vec<(&str, bool)> = target
        .registrations
        .iter()
        .map(|row| (row.pattern_id.as_str(), row.by_name))
        .collect();
    assert_eq!(
        lines,
        [("demo.route.v1", true); 6]
            .into_iter()
            .chain([("demo.router_mount.v1", true)])
            .collect::<Vec<_>>(),
        "{:?}",
        target.registrations
    );
    let text = format_reference_report(&target, 20);
    assert!(text.contains("- route /view_target [app.py:"), "{text}");
    assert!(
        text.contains("- demo.router_mount.v1 [app.py:") && !text.contains("/registrar"),
        "{text}"
    );
    assert!(
        format_reference_report(&report(&repo, "other", None), 20).contains("Registrations (3):"),
    );
}

#[test]
fn a_class_of_the_handler_name_gets_no_row_from_a_file_that_defines_the_name() {
    let repo = scanned_repo(&[FLASK_VIEW, ("models.py", "class target:\n    pass\n")]);

    let class = report(&repo, "target", Some("models.py"));

    assert!(class.registrations.is_empty(), "{:?}", class.registrations);
}

#[test]
fn a_class_that_a_route_fact_names_in_a_file_without_its_definition_is_registered_by_name() {
    let repo = scanned_repo(&[
        ("app.py", "def other():\n    return 1\n"),
        ("models.py", "class Target:\n    pass\n"),
    ]);
    insert_fact(&repo, "demo.route.v1", None, r#"{"handler":"Target"}"#);

    let class = report(&repo, "Target", Some("models.py"));

    let rows: Vec<(&str, bool)> = class
        .registrations
        .iter()
        .map(|row| (row.pattern_id.as_str(), row.by_name))
        .collect();
    assert_eq!(rows, [("demo.route.v1", true)], "{}", dump(&class));
}

#[test]
fn a_django_view_is_registered_by_the_last_segment_of_its_dotted_view_target() {
    let repo = scanned_repo(&[
        (
            "shop/urls.py",
            "from django.urls import path\nfrom . import views\n\nurlpatterns = [\n    path(\"cart/\", views.cart_view, name=\"cart\"),\n    path(\"pay/\", views.pay),\n]\n",
        ),
        (
            "shop/views.py",
            "def cart_view(request):\n    return None\n\n\ndef pay(request):\n    return None\n",
        ),
    ]);
    let cart = line_of(&repo, "shop/urls.py", "views.cart_view");
    let pay = line_of(&repo, "shop/urls.py", "views.pay");

    let cart_view = report(&repo, "cart_view", None);
    let pay_view = report(&repo, "pay", None);

    assert_eq!(
        registered(&cart_view),
        [format!(
            "django.url_pattern.v1 shop/urls.py:{cart} (by name)"
        )],
        "{}",
        dump(&cart_view)
    );
    assert_eq!(
        registered(&pay_view),
        [format!(
            "django.url_pattern.v1 shop/urls.py:{pay} (by name)"
        )],
        "{}",
        dump(&pay_view)
    );
    assert!(format_reference_report(&pay_view, 20).contains(&format!(
        "- django.url_pattern.v1 [shop/urls.py:{pay}] (by name)\n"
    )));
}

#[test]
fn a_dotted_handler_value_matches_only_by_the_whole_last_segment() {
    let repo = scanned_repo(&[("app.py", "def target():\n    return 1\n")]);
    for handler in [
        "pkg.views.target",
        "pkg.views.mytarget",
        "pkg._target",
        "mytarget",
        "target.run",
        "pkg.Target",
    ] {
        insert_fact(
            &repo,
            "demo.route.v1",
            None,
            &format!(r#"{{"handler":"{handler}","route_template":"{handler}"}}"#),
        );
    }
    insert_fact(
        &repo,
        "demo.router_mount.v1",
        None,
        r#"{"handler_names":["pkg.mod.other","pkg.mod.target"],"route_template":"mount"}"#,
    );

    let target = report(&repo, "target", None);

    let templates: Vec<&str> = target
        .registrations
        .iter()
        .map(|row| row.template.as_deref().unwrap_or_default())
        .collect();
    assert_eq!(
        templates,
        ["pkg.views.target", "mount"],
        "{:?}",
        target.registrations
    );
}

const SPRING_CONTROLLER: &str = "import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping(\"/api\")
public class Ctl {
    @GetMapping(\"/hello\")
    public String hello() { return \"hi\"; }

    public String helper() { return \"h\"; }
}
";

const ASPNET_CONTROLLER: &str = "using Microsoft.AspNetCore.Mvc;

[ApiController]
[Route(\"api/items\")]
public class ItemsController : ControllerBase
{
    [HttpGet(\"list\")]
    public IActionResult List() { return Ok(); }

    public IActionResult Helper() { return Ok(); }
}
";

const ACTIX_SERVICE: &str = "use actix_web::{get, web, App};

#[get(\"/hello\")]
async fn hello() -> &'static str {
    \"hi\"
}

fn main() {
    let _ = App::new().route(\"/ping\", web::get().to(ping));
}

async fn ping() -> &'static str {
    \"pong\"
}
";

#[test]
fn a_spring_annotation_registers_the_method_and_the_class_it_marks_by_id() {
    let repo = scanned_repo(&[("Ctl.java", SPRING_CONTROLLER)]);

    let hello = report(&repo, "hello", None);
    let class = report(&repo, "Ctl", None);
    let helper = report(&repo, "helper", None);

    assert_eq!(
        registered(&hello),
        [format!(
            "spring.request_mapping.v1 Ctl.java:{}",
            line_of(&repo, "Ctl.java", "String hello")
        )],
        "{}",
        dump(&hello)
    );
    assert_eq!(
        registered(&class),
        [format!(
            "spring.request_mapping.v1 Ctl.java:{}",
            line_of(&repo, "Ctl.java", "public class Ctl")
        )],
        "{}",
        dump(&class)
    );
    assert!(helper.registrations.is_empty(), "{}", dump(&helper));
}

#[test]
fn an_aspnet_attribute_registers_the_action_it_marks_by_id() {
    let repo = scanned_repo(&[("Controllers/ItemsController.cs", ASPNET_CONTROLLER)]);

    let list = report(&repo, "List", None);
    let helper = report(&repo, "Helper", None);

    assert_eq!(
        registered(&list),
        [format!(
            "aspnet.attribute_route.v1 Controllers/ItemsController.cs:{}",
            line_of(&repo, "Controllers/ItemsController.cs", "[HttpGet")
        )],
        "{}",
        dump(&list)
    );
    assert!(helper.registrations.is_empty(), "{}", dump(&helper));
}

#[test]
fn an_actix_attribute_registers_the_function_it_marks_but_a_call_route_does_not_register_its_caller()
 {
    let repo = scanned_repo(&[("src/main.rs", ACTIX_SERVICE)]);

    let hello = report(&repo, "hello", None);
    let main = report(&repo, "main", None);

    assert_eq!(
        registered(&hello),
        [format!(
            "actix.attribute_route.v1 src/main.rs:{}",
            line_of(&repo, "src/main.rs", "async fn hello")
        )],
        "{}",
        dump(&hello)
    );
    assert!(main.registrations.is_empty(), "{}", dump(&main));
}

#[test]
fn registration_paths_use_forward_slashes_even_when_the_index_stores_backslashes() {
    let repo = scanned_repo(&[("app.py", "def target():\n    return 1\n")]);
    insert_fact(&repo, "demo.route.v1", None, r#"{"handler":"target"}"#);
    rusqlite::Connection::open(&repo.db)
        .unwrap()
        .execute(
            "UPDATE structural_facts SET path = 'web\\urls.py' WHERE pattern_id = 'demo.route.v1'",
            [],
        )
        .unwrap();

    let target = report(&repo, "target", None);

    let paths: Vec<&str> = target
        .registrations
        .iter()
        .map(|row| row.path.as_str())
        .collect();
    assert_eq!(paths, ["web/urls.py"], "{:?}", target.registrations);
}

#[test]
fn override_paths_use_forward_slashes_even_when_the_index_stores_backslashes() {
    let repo = scanned_repo(&[
        (
            "base.py",
            "class Base:\n    def run(self):\n        return 1\n",
        ),
        (
            "pkg/child.py",
            "from base import Base\n\n\nclass Child(Base):\n    def run(self):\n        return 2\n",
        ),
    ]);
    rusqlite::Connection::open(&repo.db)
        .unwrap()
        .execute(
            "UPDATE symbols SET path = 'pkg\\child.py' WHERE path = 'pkg/child.py'",
            [],
        )
        .unwrap();

    let base = report(&repo, "run", Some("base.py"));

    let paths: Vec<&str> = base.overrides.iter().map(|row| row.path.as_str()).collect();
    assert_eq!(paths, ["pkg/child.py"], "{}", dump(&base));
}

const PY_TESTED: &[(&str, &str)] = &[
    ("lib.py", "def target():\n    return 1\n"),
    (
        "app.py",
        "from lib import target\n\n\ndef run():\n    return target()\n",
    ),
    (
        "tests/test_a.py",
        "import pytest\nfrom lib import target\n\n\ndef test_plain():\n    assert target() == 1\n\n\nclass TestKlass:\n    def setup_method(self):\n        target()\n\n    def test_method(self):\n        assert target() == 1\n\n    def helper_in_class(self):\n        return target()\n\n\n@pytest.fixture\ndef fx():\n    return target()\n\n\ndef test_with_helper():\n    def inner():\n        return target()\n\n    assert inner()\n",
    ),
];

fn running(report: &ReferenceReport) -> Vec<String> {
    report
        .tests
        .iter()
        .map(|test| {
            format!(
                "{} {}:{} {}",
                test.name,
                test.path,
                test.line,
                test.command.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

#[test]
fn python_tests_that_call_the_target_list_with_their_pytest_commands() {
    let repo = scanned_repo(PY_TESTED);
    let plain = line_of(&repo, "tests/test_a.py", "def test_plain");
    let method = line_of(&repo, "tests/test_a.py", "def test_method");
    let helper = line_of(&repo, "tests/test_a.py", "def test_with_helper");

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!("test_plain tests/test_a.py:{plain} pytest tests/test_a.py::test_plain"),
            format!(
                "test_method tests/test_a.py:{method} pytest tests/test_a.py::TestKlass::test_method"
            ),
            format!(
                "test_with_helper tests/test_a.py:{helper} pytest tests/test_a.py::test_with_helper"
            ),
        ],
        "{}",
        dump(&target)
    );
    let text = format_reference_report(&target, 20);
    assert!(
        text.contains(&format!(
            "Tests that call or use it (3):\n\
             - `test_plain` [tests/test_a.py:{plain}] run: pytest tests/test_a.py::test_plain\n\
             - `test_method` [tests/test_a.py:{method}] run: pytest tests/test_a.py::TestKlass::test_method\n\
             - `test_with_helper` [tests/test_a.py:{helper}] run: pytest tests/test_a.py::test_with_helper\n"
        )),
        "{text}"
    );
}

#[test]
fn setup_fixture_helper_and_production_callers_are_not_tests() {
    let repo = scanned_repo(PY_TESTED);

    let target = report(&repo, "target", None);

    let names: Vec<&str> = target.tests.iter().map(|test| test.name.as_str()).collect();
    for caller in ["setup_method", "fx", "helper_in_class", "inner", "run"] {
        assert!(!names.contains(&caller), "{caller} in {names:?}");
        assert!(
            target
                .sites
                .iter()
                .any(|site| site.site.from_symbol_name == caller),
            "{caller} is a site\n{}",
            dump(&target)
        );
    }
}

#[test]
fn a_helper_inside_a_test_maps_to_that_test_once() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_b.py",
            "from lib import target\n\n\ndef test_both():\n    def inner():\n        return target()\n\n    return target() + inner()\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [format!(
            "test_both tests/test_b.py:{} pytest tests/test_b.py::test_both",
            line_of(&repo, "tests/test_b.py", "def test_both")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_helper_inside_a_setup_member_is_not_a_test() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_c.py",
            "from lib import target\n\n\nclass TestC:\n    def setup_method(self):\n        def inner():\n            return target()\n\n        inner()\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert!(target.tests.is_empty(), "{}", dump(&target));
}

#[test]
fn a_python_test_in_a_nested_class_lists_every_enclosing_class_in_its_command() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_n.py",
            "from lib import target\n\n\nclass TestOuter:\n    def test_top(self):\n        assert target() == 1\n\n    class TestInner:\n        def test_deep(self):\n            assert target() == 1\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "test_top tests/test_n.py:{} pytest tests/test_n.py::TestOuter::test_top",
                line_of(&repo, "tests/test_n.py", "def test_top")
            ),
            format!(
                "test_deep tests/test_n.py:{} pytest tests/test_n.py::TestOuter::TestInner::test_deep",
                line_of(&repo, "tests/test_n.py", "def test_deep")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_python_function_named_like_a_test_inside_a_test_folds_into_that_test() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_n.py",
            "from lib import target\n\n\ndef test_outer_fn():\n    def test_helper():\n        return target()\n\n    assert test_helper() == 1\n\n\nclass TestKlass:\n    def test_method(self):\n        def test_inner():\n            return target()\n\n        assert test_inner() == 1\n\n\ndef plain():\n    def test_orphan():\n        return target()\n\n    return test_orphan()\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "test_outer_fn tests/test_n.py:{} pytest tests/test_n.py::test_outer_fn",
                line_of(&repo, "tests/test_n.py", "def test_outer_fn")
            ),
            format!(
                "test_method tests/test_n.py:{} pytest tests/test_n.py::TestKlass::test_method",
                line_of(&repo, "tests/test_n.py", "def test_method")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_python_class_inside_a_test_function_folds_into_that_test() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_f.py",
            "from lib import target\n\n\ndef test_defines_class():\n    class TestInFunc:\n        class TestDeeper:\n            def test_z(self):\n                return target()\n\n        def test_y(self):\n            return target()\n\n    assert TestInFunc().test_y() == 1\n\n\ndef plain_helper():\n    class TestOrphan:\n        def test_x(self):\n            return target()\n\n    return TestOrphan().test_x()\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [format!(
            "test_defines_class tests/test_f.py:{} pytest tests/test_f.py::test_defines_class",
            line_of(&repo, "tests/test_f.py", "def test_defines_class")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_helper_method_of_a_class_inside_a_test_function_folds_into_that_test() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/test_m.py",
            "from lib import target\n\n\ndef test_module():\n    class Module:\n        @staticmethod\n        def create():\n            return target()\n\n    assert Module.create() == 1\n\n\ndef plain_helper():\n    class Orphan:\n        def build(self):\n            return target()\n\n    return Orphan().build()\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [format!(
            "test_module tests/test_m.py:{} pytest tests/test_m.py::test_module",
            line_of(&repo, "tests/test_m.py", "def test_module")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_python_test_in_a_class_pytest_does_not_collect_lists_without_a_command() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "tests/base_case.py",
            "import unittest\n\n\nclass BaseCase(unittest.TestCase):\n    pass\n",
        ),
        (
            "tests/test_g.py",
            "import unittest\nfrom base_case import BaseCase\nfrom lib import target\n\n\nclass Helper:\n    def test_in_plain(self):\n        assert target() == 1\n\n\nclass FooTests(unittest.TestCase):\n    def test_a(self):\n        self.assertEqual(target(), 1)\n\n    class Nested(unittest.TestCase):\n        def test_nested_case(self):\n            self.assertEqual(target(), 1)\n\n\nclass BarTests(BaseCase):\n    def test_b(self):\n        self.assertEqual(target(), 1)\n\n\nclass TestOuter:\n    class Helper:\n        class TestInner:\n            def test_unreachable(self):\n                assert target() == 1\n\n    class Inner(unittest.TestCase):\n        def test_inner_case(self):\n            self.assertEqual(target(), 1)\n",
        ),
    ]);
    let line = |needle: &str| line_of(&repo, "tests/test_g.py", needle);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "test_in_plain tests/test_g.py:{} -",
                line("def test_in_plain")
            ),
            format!(
                "test_a tests/test_g.py:{} pytest tests/test_g.py::FooTests::test_a",
                line("def test_a")
            ),
            format!(
                "test_nested_case tests/test_g.py:{} -",
                line("def test_nested_case")
            ),
            format!(
                "test_b tests/test_g.py:{} pytest tests/test_g.py::BarTests::test_b",
                line("def test_b")
            ),
            format!(
                "test_unreachable tests/test_g.py:{} -",
                line("def test_unreachable")
            ),
            format!(
                "test_inner_case tests/test_g.py:{} pytest tests/test_g.py::TestOuter::Inner::test_inner_case",
                line("def test_inner_case")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_test_whose_only_site_the_report_excludes_is_not_listed() {
    let repo = scanned_repo(&[
        ("lib.py", "def target():\n    return 1\n"),
        (
            "src/test/java/FooTest.java",
            "package x;\n\nimport org.junit.Test;\n\npublic class FooTest {\n    @Test\n    public void checks() {\n        Foo.target();\n    }\n}\n",
        ),
    ]);

    let target = report(&repo, "target", Some("lib.py"));

    assert_eq!(target.counts.excluded, 1, "{}", dump(&target));
    assert!(target.tests.is_empty(), "{}", dump(&target));
}

#[test]
fn rust_unit_and_integration_tests_list_with_cargo_commands() {
    let repo = scanned_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn target() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn unit_case() {\n        assert_eq!(target(), 1);\n    }\n}\n",
        ),
        (
            "tests/it.rs",
            "use my_crate::target;\n\n#[test]\nfn integration_case() {\n    assert_eq!(target(), 1);\n}\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "unit_case src/lib.rs:{} cargo test -p my-crate unit_case",
                line_of(&repo, "src/lib.rs", "fn unit_case")
            ),
            format!(
                "integration_case tests/it.rs:{} cargo test -p my-crate --test it integration_case",
                line_of(&repo, "tests/it.rs", "fn integration_case")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_rust_test_without_a_cargo_toml_runs_by_name() {
    let repo = scanned_repo(&[(
        "src/lib.rs",
        "pub fn target() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn unit_case() {\n        assert_eq!(target(), 1);\n    }\n}\n",
    )]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [format!(
            "unit_case src/lib.rs:{} cargo test unit_case",
            line_of(&repo, "src/lib.rs", "fn unit_case")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_rust_test_marked_ignore_lists_the_ignored_flag_in_its_command() {
    let repo = scanned_repo(&[
        (
            "Cargo.toml",
            "[package]\nname = \"my-crate\"\nversion = \"0.1.0\"\n",
        ),
        (
            "src/lib.rs",
            "pub fn target() -> i32 {\n    1\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    #[ignore = \"slow\"]\n    fn slow_case() {\n        assert_eq!(target(), 1);\n    }\n\n    #[test]\n    fn quick_case() {\n        assert_eq!(target(), 1);\n    }\n}\n",
        ),
        (
            "tests/it.rs",
            "use my_crate::target;\n\n#[test]\n#[ignore]\nfn slow_integration() {\n    assert_eq!(target(), 1);\n}\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "slow_case src/lib.rs:{} cargo test -p my-crate slow_case -- --ignored",
                line_of(&repo, "src/lib.rs", "fn slow_case")
            ),
            format!(
                "quick_case src/lib.rs:{} cargo test -p my-crate quick_case",
                line_of(&repo, "src/lib.rs", "fn quick_case")
            ),
            format!(
                "slow_integration tests/it.rs:{} cargo test -p my-crate --test it slow_integration -- --ignored",
                line_of(&repo, "tests/it.rs", "fn slow_integration")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn java_tests_in_a_build_folder_below_the_root_list_the_cd_prefix() {
    let main = "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n";
    let test = "package x;\n\nimport org.junit.Test;\n\npublic class FooTest {\n    @Test\n    public void checksTarget() {\n        Foo.target();\n    }\n}\n";
    let cases = [
        (
            vec![("mvn/pom.xml", "<project></project>\n")],
            "mvn",
            "cd mvn && mvn test -Dtest=FooTest#checksTarget",
        ),
        (
            vec![("gr/gradlew", ""), ("gr/build.gradle", "")],
            "gr",
            "cd gr && ./gradlew test --tests 'FooTest.checksTarget'",
        ),
        (
            vec![("gradlew", ""), ("gr/build.gradle", "")],
            "gr",
            "cd gr && ../gradlew test --tests 'FooTest.checksTarget'",
        ),
    ];
    for (markers, dir, command) in cases {
        let main_path = format!("{dir}/src/main/java/x/Foo.java");
        let test_path = format!("{dir}/src/test/java/x/FooTest.java");
        let files: Vec<(&str, &str)> = markers
            .into_iter()
            .chain([(main_path.as_str(), main), (test_path.as_str(), test)])
            .collect();
        let repo = scanned_repo(&files);

        let target = report(&repo, "target", None);

        assert_eq!(
            running(&target),
            [format!(
                "checksTarget {test_path}:{} {command}",
                line_of(&repo, &test_path, "@Test")
            )],
            "{}",
            dump(&target)
        );
    }
}

#[test]
fn a_go_test_lists_with_its_folder_relative_to_go_mod() {
    let repo = scanned_repo(&[
        ("go.mod", "module example.com/m\n"),
        (
            "pkg/foo.go",
            "package pkg\n\nfunc Target() int {\n\treturn 1\n}\n",
        ),
        (
            "pkg/foo_test.go",
            "package pkg\n\nimport \"testing\"\n\nfunc TestTarget(t *testing.T) {\n\tif Target() != 1 {\n\t\tt.Fatal(\"x\")\n\t}\n}\n",
        ),
    ]);

    let target = report(&repo, "Target", None);

    assert_eq!(
        running(&target),
        [format!(
            "TestTarget pkg/foo_test.go:{} go test ./pkg -run '^TestTarget$'",
            line_of(&repo, "pkg/foo_test.go", "func TestTarget")
        )],
        "{}",
        dump(&target)
    );
}

const GO_TESTED: &[(&str, &str)] = &[
    ("go.mod", "module example.com/m\n"),
    (
        "pkg/foo.go",
        "package pkg\n\nfunc Target() int {\n\treturn 1\n}\n",
    ),
    (
        "pkg/foo_test.go",
        "package pkg\n\nimport (\n\t\"fmt\"\n\t\"testing\"\n)\n\nfunc TestPlain(t *testing.T) {\n\tTarget()\n}\n\nfunc TestSub(t *testing.T) {\n\tt.Run(\"case a\", func(t *testing.T) {\n\t\tTarget()\n\t})\n\tt.Run(\"case b\", func(t *testing.T) {\n\t\tTarget()\n\t})\n}\n\nfunc BenchmarkTarget(b *testing.B) {\n\tTarget()\n}\n\nfunc ExampleTarget() {\n\tfmt.Println(Target())\n\t// Output: 1\n}\n\ntype Suite struct{}\n\nfunc (s *Suite) TestSuiteMethod() {\n\tTarget()\n}\n\nfunc helperNotATest(t *testing.T) {\n\tTarget()\n}\n",
    ),
];

#[test]
fn a_go_subtest_closure_folds_into_its_test_function_once() {
    let repo = scanned_repo(GO_TESTED);

    let target = report(&repo, "Target", None);

    let sub: Vec<String> = running(&target)
        .into_iter()
        .filter(|row| row.contains("TestSub") || row.contains("case"))
        .collect();
    assert_eq!(
        sub,
        [format!(
            "TestSub pkg/foo_test.go:{} go test ./pkg -run '^TestSub$'",
            line_of(&repo, "pkg/foo_test.go", "func TestSub")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn go_benchmarks_run_with_the_bench_flag_and_functions_go_test_skips_list_without_a_command() {
    let repo = scanned_repo(GO_TESTED);
    let line = |needle: &str| line_of(&repo, "pkg/foo_test.go", needle);

    let target = report(&repo, "Target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "TestPlain pkg/foo_test.go:{} go test ./pkg -run '^TestPlain$'",
                line("func TestPlain")
            ),
            format!(
                "TestSub pkg/foo_test.go:{} go test ./pkg -run '^TestSub$'",
                line("func TestSub")
            ),
            format!(
                "BenchmarkTarget pkg/foo_test.go:{} go test ./pkg -bench '^BenchmarkTarget$' -run '^$'",
                line("func BenchmarkTarget")
            ),
            format!(
                "ExampleTarget pkg/foo_test.go:{} go test ./pkg -run '^ExampleTarget$'",
                line("func ExampleTarget")
            ),
            format!(
                "TestSuiteMethod pkg/foo_test.go:{} -",
                line("TestSuiteMethod()")
            ),
            format!(
                "helperNotATest pkg/foo_test.go:{} -",
                line("func helperNotATest")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_go_subtest_closure_inside_a_helper_that_is_not_a_test_is_not_listed() {
    let repo = scanned_repo(&[
        ("go.mod", "module example.com/m\n"),
        (
            "pkg/foo.go",
            "package pkg\n\nfunc Target() int {\n\treturn 1\n}\n",
        ),
        (
            "pkg/foo_test.go",
            "package pkg\n\nimport \"testing\"\n\nfunc runCases(t *testing.T) {\n\tt.Run(\"case a\", func(t *testing.T) {\n\t\tTarget()\n\t})\n}\n",
        ),
    ]);

    let target = report(&repo, "Target", None);

    assert!(target.tests.is_empty(), "{}", dump(&target));
}

#[test]
fn script_tests_list_with_the_runner_of_the_nearest_package_json() {
    let repo = scanned_repo(&[
        (
            "web/package.json",
            "{\"name\":\"web\",\"devDependencies\":{\"vitest\":\"^1.0.0\"}}\n",
        ),
        ("web/lib.js", "export function target() {\n  return 1;\n}\n"),
        (
            "web/a.test.js",
            "import { target } from './lib.js';\n\ndescribe('suite', () => {\n  it('does \"quoted\" y', () => {\n    expect(target()).toBe(1);\n  });\n});\n\ntest('top level', () => {\n  expect(target()).toBe(1);\n});\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "does \"quoted\" y web/a.test.js:{} cd web && npx vitest run a.test.js -t \"does \\\"quoted\\\" y\"",
                line_of(&repo, "web/a.test.js", "it('does")
            ),
            format!(
                "top level web/a.test.js:{} cd web && npx vitest run a.test.js -t \"top level\"",
                line_of(&repo, "web/a.test.js", "test('top level'")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_script_test_without_a_known_runner_lists_without_a_command() {
    let repo = scanned_repo(&[
        ("lib.js", "export function target() {\n  return 1;\n}\n"),
        (
            "a.test.js",
            "import { target } from './lib.js';\n\ntest('top level', () => {\n  expect(target()).toBe(1);\n});\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    let line = line_of(&repo, "a.test.js", "test('top level'");
    assert_eq!(
        running(&target),
        [format!("top level a.test.js:{line} -")],
        "{}",
        dump(&target)
    );
    let text = format_reference_report(&target, 20);
    assert!(
        text.contains(&format!("- `top level` [a.test.js:{line}]\n")),
        "{text}"
    );
    let json = serde_json::to_value(&target).unwrap();
    assert!(json["tests"][0].get("command").is_none(), "{json}");
}

#[test]
fn java_and_csharp_tests_list_with_their_build_tool_commands() {
    let java = scanned_repo(&[
        ("pom.xml", "<project></project>\n"),
        (
            "src/main/java/x/Foo.java",
            "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n",
        ),
        (
            "src/test/java/x/FooTest.java",
            "package x;\n\nimport org.junit.Test;\n\npublic class FooTest {\n    @Test\n    public void checksTarget() {\n        Foo.target();\n    }\n\n    @org.junit.Before\n    public void setUp() {\n        Foo.target();\n    }\n}\n",
        ),
    ]);
    let csharp = scanned_repo(&[
        (
            "Foo.cs",
            "public class Foo\n{\n    public static int Target()\n    {\n        return 1;\n    }\n}\n",
        ),
        (
            "tests/FooTests.cs",
            "using Xunit;\n\npublic class FooTests\n{\n    [Fact]\n    public void ChecksTarget()\n    {\n        Foo.Target();\n    }\n}\n",
        ),
    ]);

    let java_report = report(&java, "target", None);
    let csharp_report = report(&csharp, "Target", None);

    assert_eq!(
        running(&java_report),
        [format!(
            "checksTarget src/test/java/x/FooTest.java:{} mvn test -Dtest=FooTest#checksTarget",
            line_of(&java, "src/test/java/x/FooTest.java", "@Test")
        )],
        "{}",
        dump(&java_report)
    );
    assert_eq!(
        running(&csharp_report),
        [format!(
            "ChecksTarget tests/FooTests.cs:{} dotnet test --filter \"FullyQualifiedName~FooTests.ChecksTarget\"",
            line_of(&csharp, "tests/FooTests.cs", "[Fact]")
        )],
        "{}",
        dump(&csharp_report)
    );
}

#[test]
fn a_java_test_in_a_nested_class_lists_the_binary_class_name_in_its_command() {
    let repo = scanned_repo(&[
        ("pom.xml", "<project></project>\n"),
        (
            "src/main/java/x/Foo.java",
            "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n",
        ),
        (
            "src/test/java/x/OuterTest.java",
            "package x;\n\nimport org.junit.jupiter.api.Nested;\nimport org.junit.jupiter.api.Test;\n\npublic class OuterTest {\n    @Test\n    void top() {\n        Foo.target();\n    }\n\n    @Nested\n    class InnerTest {\n        @Test\n        void checks() {\n            Foo.target();\n        }\n    }\n}\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [
            format!(
                "top src/test/java/x/OuterTest.java:{} mvn test -Dtest=OuterTest#top",
                line_of(&repo, "src/test/java/x/OuterTest.java", "@Test")
            ),
            format!(
                "checks src/test/java/x/OuterTest.java:{} mvn test -Dtest='OuterTest$InnerTest'#checks",
                line_of(&repo, "src/test/java/x/OuterTest.java", "        @Test")
            ),
        ],
        "{}",
        dump(&target)
    );
}

#[test]
fn a_test_in_another_language_lists_without_a_command() {
    let repo = scanned_repo(&[
        ("lib/foo.rb", "def target\n  1\nend\n"),
        (
            "test/foo_test.rb",
            "require_relative '../lib/foo'\n\nclass FooTest\n  def test_target\n    target\n  end\nend\n",
        ),
    ]);

    let target = report(&repo, "target", None);

    assert_eq!(
        running(&target),
        [format!(
            "test_target test/foo_test.rb:{} -",
            line_of(&repo, "test/foo_test.rb", "def test_target")
        )],
        "{}",
        dump(&target)
    );
}

#[test]
fn test_rows_fold_after_the_limit_with_the_full_count() {
    let repo = scanned_repo(PY_TESTED);
    let plain = line_of(&repo, "tests/test_a.py", "def test_plain");

    let target = report(&repo, "target", None);
    let text = format_reference_report(&target, 1);

    assert_eq!(target.tests.len(), 3);
    assert!(
        text.contains(&format!(
            "Tests that call or use it (3):\n\
             - `test_plain` [tests/test_a.py:{plain}] run: pytest tests/test_a.py::test_plain\n\
             … 2 more tests in 1 file: tests/test_a.py (2)\n"
        )),
        "{text}"
    );
    let json = serde_json::to_value(&target).unwrap();
    assert_eq!(json["tests"].as_array().unwrap().len(), 3);
    assert_eq!(json["tests"][0]["name"], "test_plain");
    assert_eq!(
        json["tests"][0]["command"],
        "pytest tests/test_a.py::test_plain"
    );
}

#[test]
fn tests_print_after_the_registrations_and_before_the_text_only_lines() {
    let repo = scanned_repo(&[
        (
            "app.py",
            "from flask import Flask\n\napp = Flask(__name__)\n\n\n@app.route(\"/x\")\ndef target():\n    return \"x\"\n",
        ),
        (
            "tests/test_app.py",
            "from app import target\n\n\ndef test_target():\n    assert target()\n",
        ),
        ("notes.md", "target is documented here\n"),
    ]);
    let decorator = line_of(&repo, "app.py", "@app.route");
    let test = line_of(&repo, "tests/test_app.py", "def test_target");

    let text = format_reference_report(&report(&repo, "target", None), 20);

    assert!(
        text.contains(&format!(
            "Registrations (1):\n\
             - route GET /x [app.py:{decorator}]\n\
             Tests that call or use it (1):\n\
             - `test_target` [tests/test_app.py:{test}] run: pytest tests/test_app.py::test_target\n\
             Text only (1): "
        )),
        "{text}"
    );
}

#[test]
fn a_test_name_with_control_characters_prints_them_as_escapes_in_text_and_unchanged_in_json() {
    let repo = scanned_repo(&[
        (
            "package.json",
            "{\"devDependencies\":{\"vitest\":\"^1\"}}\n",
        ),
        ("lib.js", "export function target() {\n  return 1;\n}\n"),
        (
            "a.test.js",
            "import { target } from './lib.js';\n\ntest(`first line\nsecond\tcolumn`, () => {\n  expect(target()).toBe(1);\n});\n",
        ),
    ]);
    let line = line_of(&repo, "a.test.js", "test(`first line");

    let target = report(&repo, "target", None);
    let text = format_reference_report(&target, 20);
    let json = serde_json::to_value(&target).unwrap();

    assert!(
        text.contains(&format!(
            "- `first line\\nsecond\\tcolumn` [a.test.js:{line}] run: npx vitest run a.test.js"
        )),
        "{text}"
    );
    assert_eq!(json["tests"][0]["name"], "first line\nsecond\tcolumn");
}

#[test]
fn a_target_no_test_calls_prints_no_test_section() {
    let repo = scanned_repo(PY_TESTED);

    let run = report(&repo, "run", None);

    assert!(run.tests.is_empty());
    assert!(!format_reference_report(&run, 20).contains("Tests that call or use it"));
    assert!(serde_json::to_value(&run).unwrap().get("tests").is_none());
}

#[test]
fn a_vitest_or_jest_name_is_a_regular_expression_and_a_mocha_name_is_not() {
    let source = "import { target } from './lib.js';\n\ntest('sums a.b (1+2) [3] $4', () => {\n  expect(target()).toBe(1);\n});\n";
    let cases = [
        (
            "{\"devDependencies\":{\"vitest\":\"^1\"}}",
            r#"npx vitest run a.test.js -t "sums a\\.b \\(1\\+2\\) \\[3\\] \\\$4""#,
        ),
        (
            "{\"devDependencies\":{\"jest\":\"^29\"}}",
            r#"npx jest --runTestsByPath a.test.js -t "sums a\\.b \\(1\\+2\\) \\[3\\] \\\$4""#,
        ),
        (
            "{\"dependencies\":{\"mocha\":\"^10\"}}",
            r#"npx mocha a.test.js --fgrep "sums a.b (1+2) [3] \$4""#,
        ),
    ];
    for (package_json, command) in cases {
        let repo = scanned_repo(&[
            ("package.json", package_json),
            ("lib.js", "export function target() {\n  return 1;\n}\n"),
            ("a.test.js", source),
        ]);

        let target = report(&repo, "target", None);

        assert_eq!(
            running(&target),
            [format!(
                "sums a.b (1+2) [3] $4 a.test.js:{} {command}",
                line_of(&repo, "a.test.js", "test('sums")
            )],
            "{}",
            dump(&target)
        );
    }
}

#[test]
fn a_template_literal_hole_in_a_test_name_and_a_space_in_the_path_reach_the_command() {
    let source = "import { target } from './lib.js';\n\nfor (const kind of ['a']) {\n  test(`handles ${kind} kind`, () => {\n    expect(target()).toBe(1);\n  });\n}\n";
    let cases = [
        (
            "{\"devDependencies\":{\"vitest\":\"^1\"}}",
            r#"npx vitest run 'a b.test.js' -t "handles .* kind""#,
        ),
        (
            "{\"dependencies\":{\"mocha\":\"^10\"}}",
            r#"npx mocha 'a b.test.js' --fgrep "handles ""#,
        ),
    ];
    for (package_json, command) in cases {
        let repo = scanned_repo(&[
            ("package.json", package_json),
            ("lib.js", "export function target() {\n  return 1;\n}\n"),
            ("a b.test.js", source),
        ]);

        let target = report(&repo, "target", None);

        assert_eq!(
            running(&target),
            [format!(
                "handles ${{kind}} kind a b.test.js:{} {command}",
                line_of(&repo, "a b.test.js", "test(`handles")
            )],
            "{}",
            dump(&target)
        );
    }
}

#[test]
fn script_tests_list_the_command_of_each_runner_a_package_json_can_name() {
    let cases = [
        (
            "{\"devDependencies\":{\"jest\":\"^29\"}}",
            "npx jest --runTestsByPath a.test.js -t \"top level\"",
        ),
        (
            "{\"dependencies\":{\"mocha\":\"^10\"}}",
            "npx mocha a.test.js --fgrep \"top level\"",
        ),
        (
            "{\"scripts\":{\"test\":\"node --test\"}}",
            "node --test-name-pattern \"^top level\\$\" a.test.js",
        ),
    ];
    for (package_json, command) in cases {
        let repo = scanned_repo(&[
            ("package.json", package_json),
            ("lib.js", "export function target() {\n  return 1;\n}\n"),
            (
                "a.test.js",
                "import { target } from './lib.js';\n\ntest('top level', () => {\n  expect(target()).toBe(1);\n});\n",
            ),
        ]);

        let target = report(&repo, "target", None);

        assert_eq!(
            running(&target),
            [format!(
                "top level a.test.js:{} {command}",
                line_of(&repo, "a.test.js", "test('top level'")
            )],
            "{package_json}\n{}",
            dump(&target)
        );
    }
}

#[test]
fn java_gradle_tests_list_the_wrapper_when_the_project_has_one() {
    let main = (
        "src/main/java/x/Foo.java",
        "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n",
    );
    let test = (
        "src/test/java/x/FooTest.java",
        "package x;\n\nimport org.junit.Test;\n\npublic class FooTest {\n    @Test\n    public void checksTarget() {\n        Foo.target();\n    }\n}\n",
    );
    let cases = [
        (vec![("gradlew", ""), ("build.gradle", "")], "./gradlew"),
        (vec![("gradlew", ""), ("build.gradle.kts", "")], "./gradlew"),
        (vec![("build.gradle", "")], "gradle"),
    ];
    for (markers, tool) in cases {
        let files: Vec<(&str, &str)> = markers.into_iter().chain([main, test]).collect();
        let repo = scanned_repo(&files);

        let target = report(&repo, "target", None);

        assert_eq!(
            running(&target),
            [format!(
                "checksTarget src/test/java/x/FooTest.java:{} {tool} test --tests 'FooTest.checksTarget'",
                line_of(&repo, "src/test/java/x/FooTest.java", "@Test")
            )],
            "{}",
            dump(&target)
        );
    }
}

const JAVA_SUPER: &[(&str, &str)] = &[
    (
        "Base.java",
        "public class Base {\n    public void f(int a) {}\n    public void g(int a) {}\n}\n",
    ),
    (
        "Sub.java",
        "public class Sub extends Base {\n    public void f(int a, int b) {}\n    @Override\n    public void g(int a) {\n        super.g(a);\n    }\n    public void run() {\n        super.f(1);\n        f(1, 2);\n    }\n}\n",
    ),
];

const JAVA_SUPER_ONE_FILE: &[(&str, &str)] = &[(
    "All.java",
    "class Base {\n    public void g(int a) {}\n}\n\nclass Sub extends Base {\n    @Override\n    public void g(int a) {\n        super.g(a);\n    }\n}\n",
)];

const KOTLIN_SUPER: &[(&str, &str)] = &[
    (
        "Base.kt",
        "open class Base {\n    open fun f(a: Int) {}\n    open fun g(a: Int) {}\n}\n",
    ),
    (
        "Sub.kt",
        "class Sub : Base() {\n    fun f(a: Int, b: Int) {}\n    override fun g(a: Int) {\n        super.g(a)\n    }\n    fun run() {\n        super.f(1)\n        f(1, 2)\n    }\n}\n",
    ),
];

#[test]
fn a_java_super_call_reaches_the_base_method_and_not_the_override_that_makes_it() {
    let repo = scanned_repo(JAVA_SUPER);
    let from_run = line_of(&repo, "Sub.java", "super.f(1)");
    let from_override = line_of(&repo, "Sub.java", "super.g(a)");

    let base_f = report(&repo, "Base.f", None);
    assert_site(
        &base_f,
        "Sub.java",
        from_run,
        SiteLabel::Proven,
        "class chain",
    );
    let base_g = report(&repo, "Base.g", None);
    assert_site(
        &base_g,
        "Sub.java",
        from_override,
        SiteLabel::Proven,
        "class chain",
    );

    let sub_g = report(&repo, "Sub.g", None);
    assert_site(
        &sub_g,
        "Sub.java",
        from_override,
        SiteLabel::Excluded,
        "resolves to `Base.g` (Base.java:3)",
    );
}

#[test]
fn a_java_super_call_in_the_file_of_its_base_class_reaches_the_base_method() {
    let repo = scanned_repo(JAVA_SUPER_ONE_FILE);
    let line = line_of(&repo, "All.java", "super.g(a)");

    let base = report(&repo, "Base.g", None);
    assert_site(&base, "All.java", line, SiteLabel::Proven, "class chain");
    let sub = report(&repo, "Sub.g", None);
    assert_site(
        &sub,
        "All.java",
        line,
        SiteLabel::Excluded,
        "resolves to `Base.g` (All.java:2)",
    );
}

#[test]
fn a_kotlin_super_call_reaches_the_base_method_and_not_the_override_that_makes_it() {
    let repo = scanned_repo(KOTLIN_SUPER);
    let from_run = line_of(&repo, "Sub.kt", "super.f(1)");
    let from_override = line_of(&repo, "Sub.kt", "super.g(a)");

    let base_f = report(&repo, "Base.f", None);
    assert_site(
        &base_f,
        "Sub.kt",
        from_run,
        SiteLabel::Proven,
        "class chain",
    );
    let base_g = report(&repo, "Base.g", None);
    assert_site(
        &base_g,
        "Sub.kt",
        from_override,
        SiteLabel::Proven,
        "class chain",
    );

    let sub_g = report(&repo, "Sub.g", None);
    assert_site(
        &sub_g,
        "Sub.kt",
        from_override,
        SiteLabel::Excluded,
        "resolves to `Base.g` (Base.kt:3)",
    );
}

#[test]
fn a_super_call_reaches_neither_its_own_class_nor_a_class_that_is_no_ancestor() {
    let repo = scanned_repo(&[
        (
            "Other.java",
            "public class Other {\n    public String name() { return \"o\"; }\n}\n",
        ),
        (
            "Sub.java",
            "public class Sub extends Thread {\n    public String name() {\n        return super.name();\n    }\n}\n",
        ),
    ]);
    let line = line_of(&repo, "Sub.java", "super.name()");
    let basis = "receiver `super` reaches only the ancestors of `Sub`";

    let other = report(&repo, "Other.name", None);
    assert_site(&other, "Sub.java", line, SiteLabel::Excluded, basis);
    let own = report(&repo, "Sub.name", None);
    assert_site(&own, "Sub.java", line, SiteLabel::Excluded, basis);
}

#[test]
fn a_typescript_receiver_typed_any_unknown_or_object_never_excludes_a_site() {
    let repo = scanned_repo(&[
        (
            "src/k.ts",
            "export class K {\n  m(a: number): number { return a; }\n}\n\nexport class Dog {\n  m(a: number): number { return a + 1; }\n}\n",
        ),
        (
            "src/use.ts",
            "import { K, Dog } from \"./k\";\n\nexport function viaAny(k: any): number {\n  return k.m(1);\n}\n\nexport function viaUnknown(k: unknown): number {\n  if (k instanceof K) {\n    return k.m(2);\n  }\n  return 0;\n}\n\nexport function viaObject(k: object): number {\n  if (k instanceof K) {\n    return k.m(3);\n  }\n  return 0;\n}\n\nexport function viaDog(d: Dog): number {\n  return d.m(4);\n}\n",
        ),
    ]);
    let report = report(&repo, "K.m", None);

    for needle in ["k.m(1)", "k.m(2)", "k.m(3)"] {
        let line = line_of(&repo, "src/use.ts", needle);
        assert_site(
            &report,
            "src/use.ts",
            line,
            SiteLabel::Unresolved,
            "2 definitions",
        );
    }
    let line = line_of(&repo, "src/use.ts", "d.m(4)");
    assert_site(
        &report,
        "src/use.ts",
        line,
        SiteLabel::Excluded,
        "resolves to `Dog.m` (src/k.ts:6)",
    );
}

#[test]
fn a_python_receiver_typed_any_or_object_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "animals.py",
        "from typing import Any\n\n\nclass Cat:\n    def purr(self, a):\n        return a\n\n\nclass Dog:\n    def purr(self, a):\n        return a + 1\n\n\ndef via_any(x: Any):\n    return x.purr(1)\n\n\ndef via_object(x: object):\n    return x.purr(2)\n\n\ndef via_dog(d: Dog):\n    return d.purr(3)\n",
    )]);
    let report = report(&repo, "Cat.purr", None);

    for needle in ["x.purr(1)", "x.purr(2)"] {
        let line = line_of(&repo, "animals.py", needle);
        assert_site(
            &report,
            "animals.py",
            line,
            SiteLabel::Unresolved,
            "2 definitions",
        );
    }
    let line = line_of(&repo, "animals.py", "d.purr(3)");
    assert_site(
        &report,
        "animals.py",
        line,
        SiteLabel::Excluded,
        "resolves to `Dog.purr` (animals.py:10)",
    );
}

#[test]
fn a_csharp_receiver_typed_dynamic_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "Program.cs",
        "public class Cat { public int Purr(int a) { return a; } }\npublic class Dog { public int Purr(int a) { return a + 1; } }\n\npublic static class Use\n{\n    public static int ViaDynamic(dynamic x) { return x.Purr(1); }\n    public static int ViaDog(Dog d) { return d.Purr(3); }\n}\n",
    )]);
    let report = report(&repo, "Cat.Purr", None);

    let line = line_of(&repo, "Program.cs", "x.Purr(1)");
    assert_site(
        &report,
        "Program.cs",
        line,
        SiteLabel::Unresolved,
        "2 definitions",
    );
    let line = line_of(&repo, "Program.cs", "d.Purr(3)");
    assert_site(
        &report,
        "Program.cs",
        line,
        SiteLabel::Excluded,
        "resolves to `Dog.Purr` (Program.cs:2)",
    );
}

#[test]
fn a_python_receiver_typed_any_through_a_module_or_an_alias_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "q.py",
        r#"import typing
import typing as t
import typing_extensions as te
from typing import Any as AnyT
from typing import List as L


class Cat:
    def purr(self, a):
        return a


class Dog:
    def purr(self, a):
        return a + 1


def via_module(x: typing.Any):
    return x.purr(1)


def via_module_alias(x: t.Any):
    return x.purr(2)


def via_extensions(x: te.Any):
    return x.purr(3)


def via_name_alias(x: AnyT):
    return x.purr(4)


def via_string(x: "typing.Any"):
    return x.purr(5)


def via_default(x: typing.Any = None):
    return x.purr(6)


def via_list(x: L):
    return x.purr(7)
"#,
    )]);
    let report = report(&repo, "Cat.purr", None);

    for needle in [
        "x.purr(1)",
        "x.purr(2)",
        "x.purr(3)",
        "x.purr(4)",
        "x.purr(5)",
        "x.purr(6)",
    ] {
        let line = line_of(&repo, "q.py", needle);
        assert_site(
            &report,
            "q.py",
            line,
            SiteLabel::Unresolved,
            "2 definitions",
        );
    }
    let line = line_of(&repo, "q.py", "x.purr(7)");
    assert_site(
        &report,
        "q.py",
        line,
        SiteLabel::Excluded,
        "receiver `x` has type `L`",
    );
}

#[test]
fn a_python_receiver_typed_by_a_type_variable_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "q.py",
        r#"from typing import TypeVar


class Cat:
    def purr(self, a):
        return a


class Dog:
    def purr(self, a):
        return a + 1


T = TypeVar("T", bound=Cat)


def via_variable(x: T):
    return x.purr(1)


def via_dog(d: Dog):
    return d.purr(2)
"#,
    )]);
    let report = report(&repo, "Cat.purr", None);

    assert_not_excluded(&report, "q.py", line_of(&repo, "q.py", "x.purr(1)"));
    assert_site(
        &report,
        "q.py",
        line_of(&repo, "q.py", "d.purr(2)"),
        SiteLabel::Excluded,
        "resolves to `Dog.purr` (q.py:10)",
    );
}

#[test]
fn an_annotation_only_python_base_member_does_not_exclude_a_runtime_child_member() {
    let repo = scanned_repo(&[(
        "app.py",
        "from typing import Literal\n\n\nclass Base:\n    response_class: Literal[\"=\"]\n\n\nclass Flask(Base):\n    response_class = type\n\n\ndef make_response(app: Base):\n    return app.response_class()\n",
    )]);
    let report = report(&repo, "Flask.response_class", None);

    assert_site(
        &report,
        "app.py",
        line_of(&repo, "app.py", "app.response_class()"),
        SiteLabel::Unresolved,
        "2 definitions",
    );
}

#[test]
fn an_assigned_python_child_member_still_excludes_its_base_member() {
    let repo = scanned_repo(&[(
        "app.py",
        "class Base:\n    response_class = object\n\n\nclass Child(Base):\n    response_class = type\n\n\ndef make_response(app: Child):\n    return app.response_class()\n",
    )]);
    let report = report(&repo, "Base.response_class", None);
    let site_line = line_of(&repo, "app.py", "app.response_class()");
    let child_line = line_of(&repo, "app.py", "response_class = type");
    let basis = format!("resolves to `Child.response_class` (app.py:{child_line})");

    assert_site(&report, "app.py", site_line, SiteLabel::Excluded, &basis);
}

#[test]
fn a_python_property_descriptor_still_excludes_a_base_member() {
    let repo = scanned_repo(&[(
        "app.py",
        "class Base:\n    response_class = object\n\n\nclass Child(Base):\n    @property\n    def response_class(self):\n        return type\n\n\ndef make_response(app: Child):\n    return app.response_class\n",
    )]);
    let report = report(&repo, "Base.response_class", None);
    let site_line = line_of(&repo, "app.py", "app.response_class");
    let child_line = line_of(&repo, "app.py", "def response_class(self)");
    let basis = format!("resolves to `Child.response_class` (app.py:{child_line})");

    assert_site(&report, "app.py", site_line, SiteLabel::Excluded, &basis);
}

#[test]
fn a_dataclass_field_remains_a_competing_python_member() {
    let repo = scanned_repo(&[(
        "app.py",
        "from dataclasses import dataclass\n\n\n@dataclass\nclass Base:\n    response_class: object\n\n\nclass Child(Base):\n    response_class = type\n\n\ndef make_response(app: Base):\n    return app.response_class()\n",
    )]);
    let report = report(&repo, "Child.response_class", None);
    let site_line = line_of(&repo, "app.py", "app.response_class()");
    let base_line = line_of(&repo, "app.py", "response_class: object");
    let basis = format!("resolves to `Base.response_class` (app.py:{base_line})");

    assert_site(&report, "app.py", site_line, SiteLabel::Excluded, &basis);
}

#[test]
fn a_typed_dict_field_remains_a_competing_python_member() {
    let repo = scanned_repo(&[(
        "app.py",
        "from typing import TypedDict\n\n\nclass Base(TypedDict):\n    response_class: object\n\n\nclass Child(Base):\n    response_class = type\n\n\ndef make_response(app: Base):\n    return app.response_class()\n",
    )]);
    let report = report(&repo, "Child.response_class", None);
    let site_line = line_of(&repo, "app.py", "app.response_class()");
    let base_line = line_of(&repo, "app.py", "response_class: object");
    let basis = format!("resolves to `Base.response_class` (app.py:{base_line})");

    assert_site(&report, "app.py", site_line, SiteLabel::Excluded, &basis);
}

#[test]
fn a_java_receiver_typed_by_a_type_parameter_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "G.java",
        r#"interface Greeter { void greet(int a); }
class Cat implements Greeter { public void greet(int a) {} }
class Other { public void greet(int a) {} }
class Box<T extends Greeter> {
    T held;
    void viaClass(T x) { x.greet(1); }
    <U extends Greeter> void viaBoth(U y, T z) { y.greet(2); z.greet(3); }
    void viaField() { held.greet(4); }
}
class Use {
    static <K, V extends Greeter> void viaSecond(K k, V v) { v.greet(5); }
    static void viaOther(Other o) { o.greet(6); }
}
"#,
    )]);
    for target in ["Cat.greet", "Greeter.greet"] {
        let report = report(&repo, target, None);
        for needle in [
            "x.greet(1)",
            "y.greet(2)",
            "z.greet(3)",
            "held.greet(4)",
            "v.greet(5)",
        ] {
            assert_not_excluded(&report, "G.java", line_of(&repo, "G.java", needle));
        }
    }
    let report = report(&repo, "Cat.greet", None);
    assert_site(
        &report,
        "G.java",
        line_of(&repo, "G.java", "o.greet(6)"),
        SiteLabel::Excluded,
        "resolves to `Other.greet` (G.java:3)",
    );
}

#[test]
fn a_kotlin_receiver_typed_by_a_type_parameter_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "g.kt",
        r#"package app

interface Greeter {
    fun greet(a: Int): Int
}

class Cat : Greeter {
    override fun greet(a: Int): Int = a
}

class Other {
    fun greet(a: Int): Int = a + 1
}

fun <T : Greeter> viaT(x: T): Int {
    return x.greet(1)
}

class Box<T : Greeter> {
    fun viaClass(x: T): Int {
        return x.greet(2)
    }
}

fun viaOther(o: Other): Int {
    return o.greet(3)
}
"#,
    )]);
    let report = report(&repo, "Cat.greet", None);
    for needle in ["x.greet(1)", "x.greet(2)"] {
        assert_not_excluded(&report, "g.kt", line_of(&repo, "g.kt", needle));
    }
    assert_site(
        &report,
        "g.kt",
        line_of(&repo, "g.kt", "o.greet(3)"),
        SiteLabel::Excluded,
        "resolves to `Other.greet` (g.kt:12)",
    );
}

#[test]
fn a_rust_receiver_typed_by_a_type_parameter_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "g.rs",
        r#"pub trait Greeter {
    fn greet(&self, a: i32) -> i32;
}

pub struct Cat;

impl Greeter for Cat {
    fn greet(&self, a: i32) -> i32 {
        a
    }
}

pub struct Other;

impl Other {
    pub fn greet(&self, a: i32) -> i32 {
        a + 1
    }
}

pub fn via_t<T: Greeter>(x: T) -> i32 {
    x.greet(1)
}

pub fn via_other(o: Other) -> i32 {
    o.greet(2)
}
"#,
    )]);
    let report = report(&repo, "Cat.greet", None);
    assert_not_excluded(&report, "g.rs", line_of(&repo, "g.rs", "x.greet(1)"));
    assert_site(
        &report,
        "g.rs",
        line_of(&repo, "g.rs", "o.greet(2)"),
        SiteLabel::Excluded,
        "resolves to `Other.greet` (g.rs:16)",
    );
}

#[test]
fn a_csharp_receiver_typed_by_a_type_parameter_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "G.cs",
        r#"public interface IGreeter { int Greet(int a); }
public class Cat : IGreeter { public int Greet(int a) { return a; } }
public class Other { public int Greet(int a) { return a + 1; } }
public class Box<T> where T : IGreeter
{
    public T Held;
    public int ViaClass(T x) { return x.Greet(1); }
    public int ViaBoth<U>(U y, T z) where U : IGreeter { return y.Greet(2) + z.Greet(3); }
    public int ViaField() { return Held.Greet(4); }
}
public static class Use
{
    public static int ViaSecond<K, V>(K k, V v) where V : IGreeter { return v.Greet(5); }
    public static int ViaOther(Other o) { return o.Greet(6); }
}
"#,
    )]);
    for target in ["Cat.Greet", "IGreeter.Greet"] {
        let report = report(&repo, target, None);
        for needle in [
            "x.Greet(1)",
            "y.Greet(2)",
            "z.Greet(3)",
            "Held.Greet(4)",
            "v.Greet(5)",
        ] {
            assert_not_excluded(&report, "G.cs", line_of(&repo, "G.cs", needle));
        }
    }
    let report = report(&repo, "Cat.Greet", None);
    assert_site(
        &report,
        "G.cs",
        line_of(&repo, "G.cs", "o.Greet(6)"),
        SiteLabel::Excluded,
        "resolves to `Other.Greet` (G.cs:3)",
    );
}

#[test]
fn a_typescript_receiver_typed_by_a_type_parameter_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "g.ts",
        r#"export class K { m(a: number): number { return a; } }
export class Other { m(a: number): number { return a + 1; } }
export class Box<T extends { m(a: number): number }> {
  viaClass(x: T): number { return x.m(1); }
  viaMethod<U extends K>(y: U): number { return y.m(2); }
}
export interface Holder<W extends K> { run(w: W): number; }
export class Impl<W extends K> implements Holder<W> {
  run(w: W): number { return w.m(3); }
}
export const arrow = <V extends K>(v: V): number => v.m(4);
export function viaFn<F extends K>(f: F, g: F[]): number { return f.m(5); }
export function viaOther(o: Other): number { return o.m(6); }
"#,
    )]);
    let report = report(&repo, "K.m", None);
    for needle in ["x.m(1)", "y.m(2)", "w.m(3)", "v.m(4)", "f.m(5)"] {
        assert_not_excluded(&report, "g.ts", line_of(&repo, "g.ts", needle));
    }
    assert_site(
        &report,
        "g.ts",
        line_of(&repo, "g.ts", "o.m(6)"),
        SiteLabel::Excluded,
        "resolves to `Other.m` (g.ts:2)",
    );
}

#[test]
fn a_typescript_receiver_typed_by_a_project_alias_of_any_unknown_or_object_never_excludes_a_site() {
    let repo = scanned_repo(&[(
        "g.ts",
        r#"export class K { m(a: number): number { return a; } }
export class Other { m(a: number): number { return a + 1; } }
type Anything = any;
type Same = object;
type Wrap = Same;
type Named = Other;
export function viaAlias(k: Anything): number { return k.m(1); }
export function viaChain(k: Wrap): number { return k.m(2); }
export function viaLater(k: Later): number { return k.m(3); }
export function viaNamed(k: Named): number { return k.m(4); }
type Later = unknown;
"#,
    )]);
    let report = report(&repo, "K.m", None);
    for needle in ["k.m(1)", "k.m(2)", "k.m(3)"] {
        assert_not_excluded(&report, "g.ts", line_of(&repo, "g.ts", needle));
    }
    assert_site(
        &report,
        "g.ts",
        line_of(&repo, "g.ts", "k.m(4)"),
        SiteLabel::Excluded,
        "receiver `k` has type `Named`",
    );
}

#[test]
fn a_typescript_super_call_reaches_the_base_method_and_not_the_override_that_makes_it() {
    let repo = scanned_repo(&[
        (
            "base.ts",
            "export class Base {\n  f(a: number): void {}\n  h(a: number): void {}\n}\n",
        ),
        (
            "sub.ts",
            "import { Base } from \"./base\";\n\nexport class Sub extends Base {\n  h(a: number): void {\n    super.h(a);\n  }\n  run(): void {\n    super.f(1);\n  }\n}\n",
        ),
    ]);
    let from_override = line_of(&repo, "sub.ts", "super.h(a)");
    let from_run = line_of(&repo, "sub.ts", "super.f(1)");

    let base_f = report(&repo, "Base.f", None);
    assert_site(
        &base_f,
        "sub.ts",
        from_run,
        SiteLabel::Proven,
        "class chain",
    );
    let base_h = report(&repo, "Base.h", None);
    assert_site(
        &base_h,
        "sub.ts",
        from_override,
        SiteLabel::Proven,
        "class chain",
    );
    let sub_h = report(&repo, "Sub.h", None);
    assert_site(
        &sub_h,
        "sub.ts",
        from_override,
        SiteLabel::Excluded,
        "resolves to `Base.h` (base.ts:3)",
    );
}

#[test]
fn a_python_super_call_reaches_the_base_method_and_not_the_override_that_makes_it() {
    let repo = scanned_repo(&[
        (
            "base.py",
            "class Base:\n    def f(self, a):\n        return a\n\n    def h(self, a):\n        return a\n",
        ),
        (
            "sub.py",
            "from base import Base\n\n\nclass Sub(Base):\n    def h(self, a):\n        return super().h(a)\n\n    def run(self):\n        return super().f(1)\n",
        ),
    ]);
    let from_override = line_of(&repo, "sub.py", "super().h(a)");
    let from_run = line_of(&repo, "sub.py", "super().f(1)");

    let base_f = report(&repo, "Base.f", None);
    assert_site(
        &base_f,
        "sub.py",
        from_run,
        SiteLabel::Proven,
        "class chain",
    );
    let base_h = report(&repo, "Base.h", None);
    assert_site(
        &base_h,
        "sub.py",
        from_override,
        SiteLabel::Proven,
        "class chain",
    );
    let sub_h = report(&repo, "Sub.h", None);
    assert_site(
        &sub_h,
        "sub.py",
        from_override,
        SiteLabel::Excluded,
        "resolves to `Base.h` (base.py:5)",
    );
}

#[test]
fn a_csharp_base_call_reaches_the_base_method_and_not_the_override_that_makes_it() {
    let repo = scanned_repo(&[(
        "All.cs",
        "public class Base\n{\n    public virtual void F(int a) {}\n}\n\npublic class Sub : Base\n{\n    public override void F(int a)\n    {\n        base.F(a);\n    }\n}\n",
    )]);
    let line = line_of(&repo, "All.cs", "base.F(a)");

    let base = report(&repo, "Base.F", None);
    assert_site(&base, "All.cs", line, SiteLabel::Proven, "same file");
    let sub = report(&repo, "Sub.F", None);
    assert_site(
        &sub,
        "All.cs",
        line,
        SiteLabel::Excluded,
        "resolves to `Base.F` (All.cs:3)",
    );
}

#[test]
fn a_kotlin_super_call_that_names_an_interface_does_not_resolve_to_its_own_class() {
    let repo = scanned_repo(&[(
        "all.kt",
        "interface Greeter {\n    fun greet(a: Int): String = \"G\"\n}\n\nclass Impl : Greeter {\n    override fun greet(a: Int): String = super<Greeter>.greet(a) + \"!\"\n}\n",
    )]);
    let line = line_of(&repo, "all.kt", "super<Greeter>.greet(a)");

    let interface = report(&repo, "Greeter.greet", None);
    assert_ne!(
        site_at(&interface, "all.kt", line).label,
        SiteLabel::Excluded,
        "{}",
        dump(&interface)
    );
    let own = report(&repo, "Impl.greet", None);
    assert_site(
        &own,
        "all.kt",
        line,
        SiteLabel::Excluded,
        "receiver `super<Greeter>` reaches only the ancestors of `Impl`",
    );
}

#[test]
fn a_kotlin_expression_bodied_override_that_calls_super_reaches_the_base_method() {
    let repo = scanned_repo(&[
        (
            "Base.kt",
            "open class Base {\n    open fun g(a: Int): Int = a\n}\n",
        ),
        (
            "Sub.kt",
            "class Sub : Base() {\n    override fun g(a: Int): Int = super.g(a) + 1\n}\n",
        ),
    ]);
    let line = line_of(&repo, "Sub.kt", "super.g(a)");

    let base = report(&repo, "Base.g", None);
    assert_site(&base, "Sub.kt", line, SiteLabel::Proven, "class chain");
}

#[test]
fn a_function_nested_in_another_python_function_is_no_rival_outside_it() {
    let repo = scanned_repo(&[
        (
            "pkg/app.py",
            "class App:\n    def hook(self, f):\n        return f\n",
        ),
        (
            "tests/test_bp.py",
            "def test_bp():\n    def hook():\n        pass\n\n    hook()\n",
        ),
        (
            "tests/test_app.py",
            "def test_app(app):\n    @app.hook\n    def foo():\n        pass\n",
        ),
    ]);
    let report = report(&repo, "hook", Some("pkg/app.py"));

    assert_eq!(
        site_at(&report, "tests/test_app.py", 2).label,
        SiteLabel::Likely,
        "{}",
        dump(&report)
    );
    assert_eq!(
        site_at(&report, "tests/test_bp.py", 5).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_csharp_type_in_a_namespace_the_use_does_not_open_is_no_rival() {
    let repo = scanned_repo(&[
        (
            "src/Tests/PerformanceTests.cs",
            "namespace App.Tests\n{\n    public class PerformanceTests\n    {\n    }\n}\n",
        ),
        (
            "src/Tests/Documentation/PerformanceTests.cs",
            "namespace App.Tests.Documentation\n{\n    public class PerformanceTests\n    {\n    }\n}\n",
        ),
        (
            "src/Console/Program.cs",
            "using System;\nusing App.Tests;\n\nnamespace App.Console\n{\n    public class Program\n    {\n        public static void Run()\n        {\n            var t = new PerformanceTests();\n        }\n    }\n}\n",
        ),
        (
            "src/Console/Docs.cs",
            "using App.Tests;\nusing App.Tests.Documentation;\n\nnamespace App.Console\n{\n    public class Docs\n    {\n        public static void Run()\n        {\n            var t = new PerformanceTests();\n        }\n    }\n}\n",
        ),
    ]);
    let report = report(
        &repo,
        "PerformanceTests",
        Some("src/Tests/PerformanceTests.cs"),
    );

    assert_ne!(
        site_at(&report, "src/Console/Program.cs", 10).label,
        SiteLabel::Unresolved,
        "{}",
        dump(&report)
    );
    assert_eq!(
        site_at(&report, "src/Console/Docs.cs", 10).label,
        SiteLabel::Unresolved,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_csharp_constant_never_takes_a_type_position_of_its_name() {
    let repo = scanned_repo(&[
        (
            "src/Utilities/Cache.cs",
            "namespace App.Utilities\n{\n    internal sealed class Cache\n    {\n    }\n}\n",
        ),
        (
            "src/Bench/Holder.cs",
            "namespace App.Bench\n{\n    public class Holder\n    {\n        private static readonly object Cache = new object();\n\n        public object Get()\n        {\n            return Cache;\n        }\n    }\n}\n",
        ),
        (
            "src/Store.cs",
            "using App.Utilities;\n\nnamespace App\n{\n    internal class Store\n    {\n        private readonly Cache _cache = new Cache();\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "Cache", Some("src/Utilities/Cache.cs"));

    assert!(
        report
            .sites
            .iter()
            .filter(|site| site.site.path == "src/Store.cs")
            .all(|site| matches!(site.label, SiteLabel::Proven | SiteLabel::Likely)),
        "{}",
        dump(&report)
    );
    assert_eq!(
        site_at(&report, "src/Bench/Holder.cs", 9).label,
        SiteLabel::Excluded,
        "{}",
        dump(&report)
    );
}

#[test]
fn a_python_receiver_imported_from_outside_the_project_is_excluded() {
    let repo = scanned_repo(&[
        ("src/flask/__init__.py", ""),
        (
            "src/flask/json/__init__.py",
            "import json as _json\n\n\ndef dumps(obj):\n    return _json.dumps(obj)\n",
        ),
        (
            "tests/test_json.py",
            "from flask import json\n\n\ndef test_dumps():\n    json.dumps(1)\n",
        ),
    ]);
    let report = report(&repo, "dumps", Some("src/flask/json/__init__.py"));

    assert_site(
        &report,
        "src/flask/json/__init__.py",
        5,
        SiteLabel::Excluded,
        "receiver `_json` is imported from `json`",
    );
    assert_not_excluded(&report, "tests/test_json.py", 5);
}

#[test]
fn a_java_receiver_imported_from_a_package_outside_the_project_is_excluded() {
    let repo = scanned_repo(&[
        (
            "src/test/java/p/common/MoreAsserts.java",
            "package p.common;\n\nimport org.junit.Assert;\n\npublic final class MoreAsserts {\n    public static void assertEquals(Object a, Object b) {\n    }\n\n    public static void check(Object a, Object b) {\n        Assert.assertEquals(a, b);\n    }\n}\n",
        ),
        (
            "src/test/java/p/Use.java",
            "package p;\n\nimport p.common.MoreAsserts;\n\npublic class Use {\n    void run() {\n        MoreAsserts.assertEquals(1, 2);\n    }\n}\n",
        ),
    ]);
    let report = report(
        &repo,
        "assertEquals",
        Some("src/test/java/p/common/MoreAsserts.java"),
    );

    assert_site(
        &report,
        "src/test/java/p/common/MoreAsserts.java",
        10,
        SiteLabel::Excluded,
        "receiver `Assert` is imported from `org.junit.Assert`",
    );
    assert_not_excluded(&report, "src/test/java/p/Use.java", 7);
}

const CSHARP_CACHE: (&str, &str) = (
    "src/Utilities/Cache.cs",
    "namespace App.Utilities\n{\n    internal class Cache\n    {\n    }\n}\n",
);
const CSHARP_CACHE_UNOPENED: (&str, &str) = (
    "src/Tests/GameObject.cs",
    "using System.Collections.Concurrent;\n\nnamespace App.Tests\n{\n    public class GameObject\n    {\n        public Cache Components;\n    }\n}\n",
);

#[test]
fn a_bare_csharp_use_of_a_type_whose_namespace_is_not_open_is_excluded() {
    let repo = scanned_repo(&[
        CSHARP_CACHE,
        CSHARP_CACHE_UNOPENED,
        (
            "src/Tests/Opened.cs",
            "using App.Utilities;\n\nnamespace App.Tests\n{\n    public class Opened\n    {\n        public Cache Components;\n    }\n}\n",
        ),
        (
            "src/Utilities/Inside.cs",
            "namespace App.Utilities.Deep\n{\n    public class Inside\n    {\n        public Cache Components;\n    }\n}\n",
        ),
        (
            "src/Tests/Aliased.cs",
            "using Cache = App.Utilities.Cache;\n\nnamespace App.Tests\n{\n    public class Aliased\n    {\n        public Cache Components;\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "Cache", Some("src/Utilities/Cache.cs"));

    assert_site(
        &report,
        "src/Tests/GameObject.cs",
        7,
        SiteLabel::Excluded,
        "namespace `App.Utilities` is not open here",
    );
    assert_not_excluded(&report, "src/Utilities/Inside.cs", 5);
    for path in ["src/Tests/Opened.cs", "src/Tests/Aliased.cs"] {
        assert_not_excluded(&report, path, 7);
    }
}

const CSHARP_FALLBACK: (&str, &str) = (
    "src/Utilities/ConcurrentDictionary.cs",
    "#if !HAVE_CONCURRENT\nnamespace App.Serialization\n{\n    internal sealed class ConcurrentDictionary<TKey, TValue>\n    {\n    }\n}\n#endif\n",
);
const CSHARP_FALLBACK_STORE: (&str, &str) = (
    "src/Utilities/Store.cs",
    "using System;\n#if HAVE_CONCURRENT\nusing System.Collections.Concurrent;\n#endif\nusing App.Serialization;\n\nnamespace App.Utilities\n{\n    internal class Store\n    {\n        private ConcurrentDictionary<string, int> _store;\n    }\n}\n",
);
const CSHARP_FALLBACK_ALWAYS: (&str, &str) = (
    "src/Utilities/Always.cs",
    "using System;\nusing System.Collections.Concurrent;\nusing App.Serialization;\n\nnamespace App.Utilities\n{\n    internal class Always\n    {\n        private ConcurrentDictionary<string, int> _always;\n    }\n}\n",
);

#[test]
fn a_csharp_use_of_an_if_not_fallback_in_a_file_that_always_opens_the_paired_namespace_is_unresolved()
 {
    let repo = scanned_repo(&[
        CSHARP_FALLBACK,
        CSHARP_FALLBACK_STORE,
        CSHARP_FALLBACK_ALWAYS,
    ]);
    let report = report(&repo, "ConcurrentDictionary", None);

    assert_site(
        &report,
        "src/Utilities/Always.cs",
        9,
        SiteLabel::Unresolved,
        "`ConcurrentDictionary` is defined only without `HAVE_CONCURRENT`, and this file always opens `System.Collections.Concurrent`, which other files open with it",
    );
    assert_eq!(
        site_at(&report, "src/Utilities/Store.cs", 11).label,
        SiteLabel::Likely
    );
}

#[test]
fn an_msbuild_using_item_opens_every_csharp_namespace() {
    let repo = scanned_repo(&[
        CSHARP_CACHE,
        CSHARP_CACHE_UNOPENED,
        (
            "src/App.csproj",
            "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <ItemGroup>\n    <Using Include=\"App.Utilities\" />\n  </ItemGroup>\n</Project>\n",
        ),
    ]);
    let report = report(&repo, "Cache", Some("src/Utilities/Cache.cs"));

    assert_not_excluded(&report, "src/Tests/GameObject.cs", 7);
}

const TS_RESULT_RECEIVERS: &[(&str, &str)] = &[
    (
        "src/mini.ts",
        "export function refine(fn: () => boolean) {\n  return fn;\n}\n",
    ),
    (
        "src/classic.ts",
        "export class ZodType {\n  refine(fn: () => boolean) {\n    return this;\n  }\n}\n\nexport function string() {\n  return new ZodType();\n}\n",
    ),
    (
        "src/use.ts",
        "import * as z from \"./mini\";\nimport { string } from \"./classic\";\n\nexport const a = string().refine(() => true);\nexport const b = string()\n  .refine(() => true);\nexport const c = z.refine(() => true);\nexport const d = require(\"./mini\").refine(() => true);\n",
    ),
];

#[test]
fn a_call_on_a_call_result_never_reaches_a_top_level_function() {
    let repo = scanned_repo(TS_RESULT_RECEIVERS);
    let function = report(&repo, "refine", Some("src/mini.ts"));

    for line in [4, 6] {
        assert_site(
            &function,
            "src/use.ts",
            line,
            SiteLabel::Excluded,
            "a call on a call result never reaches the top-level `refine`",
        );
    }
    assert_not_excluded(&function, "src/use.ts", 7);
    assert_not_excluded(&function, "src/use.ts", 8);

    let method = report(&repo, "ZodType.refine", None);
    assert_eq!(
        site_at(&method, "src/use.ts", 4).label,
        SiteLabel::Likely,
        "{}",
        dump(&method)
    );
}

#[test]
fn a_call_result_reaches_a_top_level_function_that_an_object_literal_holds() {
    let mut files = TS_RESULT_RECEIVERS.to_vec();
    files.push((
        "src/api.ts",
        "import { refine } from \"./mini\";\n\nexport function api() {\n  return { refine };\n}\n\nexport const e = api().refine(() => true);\n",
    ));
    let repo = scanned_repo(&files);
    let report = report(&repo, "refine", Some("src/mini.ts"));

    assert_not_excluded(&report, "src/api.ts", 7);
    assert_not_excluded(&report, "src/use.ts", 4);
}

#[test]
fn a_const_that_holds_a_call_result_never_reaches_a_top_level_function() {
    let mut files = TS_RESULT_RECEIVERS.to_vec();
    files.push((
        "src/vars.ts",
        "import * as z from \"./mini\";\nimport { string } from \"./classic\";\n\nexport function f() {\n  const schema = string();\n  return schema.refine(() => true);\n}\n\nexport function g() {\n  const alias = z;\n  return alias.refine(() => true);\n}\n",
    ));
    let repo = scanned_repo(&files);
    let report = report(&repo, "refine", Some("src/mini.ts"));

    assert_site(
        &report,
        "src/vars.ts",
        6,
        SiteLabel::Excluded,
        "a call on a call result never reaches the top-level `refine`",
    );
    assert_not_excluded(&report, "src/vars.ts", 11);
}

#[test]
fn a_go_receiver_typed_with_the_package_of_the_owner_or_an_embedding_type_is_not_excluded() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "geom/geom.go",
            "package geom\n\ntype Point struct {\n\tX int\n}\n\nfunc (p *Point) Norm(k int) int {\n\treturn p.X * k\n}\n\ntype Wrapper struct {\n\t*Point\n}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport (\n\t\"bytes\"\n\n\t\"fx/geom\"\n)\n\nfunc ByValue() int {\n\tp := geom.Point{X: 1}\n\treturn p.Norm(3)\n}\n\nfunc ByPointer(q *geom.Point) int {\n\treturn q.Norm(1)\n}\n\nfunc ByWrapper(w geom.Wrapper) int {\n\treturn w.Norm(2)\n}\n\nfunc ByBuffer(b bytes.Buffer) int {\n\treturn b.Norm(2)\n}\n",
        ),
    ]);
    let report = report(&repo, "Point.Norm", None);

    for needle in ["return p.Norm(3)", "return q.Norm(1)", "return w.Norm(2)"] {
        assert_not_excluded(&report, "use/use.go", line_of(&repo, "use/use.go", needle));
    }
    assert_site(
        &report,
        "use/use.go",
        line_of(&repo, "use/use.go", "return b.Norm(2)"),
        SiteLabel::Excluded,
        "receiver `b` has type `bytes.Buffer`",
    );
}

#[test]
fn a_go_receiver_typed_with_an_alias_of_the_owner_is_not_excluded() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "geom/geom.go",
            "package geom\n\ntype Point struct {\n\tX int\n}\n\nfunc (p *Point) Norm(k int) int {\n\treturn p.X * k\n}\n\ntype Alias = Point\n\ntype Named Point\n\nfunc ViaAlias(a *Alias) int {\n\treturn a.Norm(2)\n}\n\nfunc ViaNamed(n Named) int {\n\treturn n.Norm(3)\n}\n",
        ),
        (
            "use/tp.go",
            "package use\n\nimport \"fx/geom\"\n\ntype Local = geom.Point\n\ntype Chain = Local\n\nfunc ViaRemoteAlias(l *Local) int {\n\treturn l.Norm(7)\n}\n\nfunc ViaVar() int {\n\tvar v Local\n\treturn v.Norm(8)\n}\n\nfunc ViaChain(c Chain) int {\n\treturn c.Norm(9)\n}\n",
        ),
    ]);
    let report = report(&repo, "Point.Norm", None);

    assert_not_excluded(
        &report,
        "geom/geom.go",
        line_of(&repo, "geom/geom.go", "return a.Norm(2)"),
    );
    for needle in ["return l.Norm(7)", "return v.Norm(8)", "return c.Norm(9)"] {
        assert_not_excluded(&report, "use/tp.go", line_of(&repo, "use/tp.go", needle));
    }
    assert_site(
        &report,
        "geom/geom.go",
        line_of(&repo, "geom/geom.go", "return n.Norm(3)"),
        SiteLabel::Excluded,
        "receiver `n` has type `Named`",
    );
}

#[test]
fn a_go_receiver_whose_package_cannot_be_the_owner_package_is_excluded() {
    let repo = scanned_repo(&[
        ("go.mod", "module fx\n\ngo 1.21\n"),
        (
            "buf/buf.go",
            "package buf\n\ntype Buffer struct{}\n\nfunc (b *Buffer) Grow(n int) {}\n",
        ),
        (
            "use/use.go",
            "package use\n\nimport (\n\t\"bytes\"\n\n\t\"fx/buf\"\n\tb2 \"fx/buf\"\n)\n\nfunc Std(b *bytes.Buffer) {\n\tb.Grow(1)\n}\n\nfunc Own(b *buf.Buffer) {\n\tb.Grow(2)\n}\n\nfunc Renamed(b *b2.Buffer) {\n\tb.Grow(3)\n}\n",
        ),
    ]);
    let report = report(&repo, "Buffer.Grow", None);

    assert_site(
        &report,
        "use/use.go",
        line_of(&repo, "use/use.go", "b.Grow(1)"),
        SiteLabel::Excluded,
        "receiver `b` has type `bytes.Buffer`",
    );
    for needle in ["b.Grow(2)", "b.Grow(3)"] {
        assert_not_excluded(&report, "use/use.go", line_of(&repo, "use/use.go", needle));
    }
}

fn listed_rows_by_section(text: &str) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    let mut section = String::new();
    for line in text.lines() {
        if let Some((title, _)) = line.split_once(" (")
            && !line.starts_with(' ')
            && !line.starts_with('-')
            && !line.starts_with('…')
        {
            section = title.to_string();
        } else if let Some(row) = line.strip_prefix("- ") {
            rows.push((section.clone(), row.to_string()));
        } else if section == "Text only"
            && let Some((path, lines)) = line.trim_start().split_once(": ")
            && line.starts_with("  ")
            && !line.trim_start().starts_with("Page offset")
        {
            for number in lines.split(", ") {
                rows.push((section.clone(), format!("{path}:{number}")));
            }
        }
    }
    rows
}

#[test]
fn paging_by_the_limit_lists_every_site_and_text_line_once() {
    let repo = scanned_repo(&[
        ("pyproject.toml", "[project]\nname = 'r'\nversion = '0.1'\n"),
        (
            "util.py",
            "class Util:\n    def helper(self):\n        return 1\n\n    def a(self):\n        return self.helper()\n\n    def b(self):\n        return self.helper()\n\n    def c(self):\n        return self.helper()\n",
        ),
        (
            "other.py",
            "class Other:\n    def helper(self):\n        return 2\n",
        ),
        (
            "use.py",
            "def d(x):\n    return x.helper()\n\n\ndef e(y):\n    return y.helper()\n",
        ),
        ("notes.py", "# helper one\n# helper two\n# helper three\n"),
        ("README.md", "helper\nhelper again\nhelper three\n"),
    ]);
    let report = report(&repo, "Util.helper", Some("util.py"));
    assert_eq!(report.counts.proven, 3, "{}", dump(&report));
    assert_eq!(report.counts.unresolved, 2, "{}", dump(&report));
    assert_eq!(report.counts.text_only_lines, 6, "{}", dump(&report));

    let limit = 2;
    let mut seen = Vec::new();
    for offset in (0..8).step_by(limit) {
        let text = code_kb_core::formatters::format_reference_report_page(&report, limit, offset);
        seen.extend(listed_rows_by_section(&text));
    }

    let mut unique = seen.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), seen.len(), "a row is listed twice: {seen:#?}");
    let count = |section: &str| seen.iter().filter(|(s, _)| s == section).count();
    assert_eq!(count("Proven"), 3, "{seen:#?}");
    assert_eq!(count("Unresolved"), 2, "{seen:#?}");
    assert_eq!(count("Text only"), 6, "{seen:#?}");
}
