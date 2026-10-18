use code_kb_core::{
    LabeledSite, ReferenceReport, SiteLabel, Workspace, find_julie_extract_binary,
    find_references_scoped, get_symbol_by_id, open_read_only, reference_report, safe_tempdir,
    scan_workspace, select_reference_target,
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
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 ORDER BY start_line LIMIT 1",
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

#[test]
fn a_javascript_read_of_the_name_is_likely_because_callback_parameters_are_not_indexed() {
    let repo = scanned_repo(&[(
        "app/index.js",
        "function count(req, res, next) {\n  next();\n}\n\napp.get('/', function (err, count) {\n  res.send({ total: count });\n});\n\napp.use(count);\n",
    )]);
    let report = report(&repo, "count", None);

    let basis = "same file; the index does not record callback parameters";
    let shadowed = line_of(&repo, "app/index.js", "total: count");
    assert_site(&report, "app/index.js", shadowed, SiteLabel::Likely, basis);
    let passed = line_of(&repo, "app/index.js", "app.use(count)");
    assert_site(&report, "app/index.js", passed, SiteLabel::Likely, basis);
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
            "package a\n\nfunc Run() {}\n\nfunc Handle() {}\n\nfunc Size() int {\n\treturn 1\n}\n",
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
            SiteLabel::Unresolved,
            "2 definitions",
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
            SiteLabel::Unresolved,
            "2 definitions",
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
fn a_paths_key_that_matches_no_indexed_file_leaves_the_module_unknown() {
    let repo = shared_name_conflict_with(&[(
        "tsconfig.json",
        "{\"compilerOptions\":{\"paths\":{\"shared/*\":[\"gone/*\"]}}}\n",
    )]);

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
fn a_callback_parameter_loop_variable_or_catch_parameter_downgrades_the_proof_to_likely() {
    let repo = scanned_repo(&[
        ("package.json", "{\"name\":\"r\"}\n"),
        ("m.ts", "export function foo() { return 1 }\n"),
        (
            "use.ts",
            "import { foo } from './m';\nexport function a(xs: any[]) { return xs.map(foo => foo()); }\nexport function b(xs: any[]) { return xs.map((n: number, foo: any) => foo()); }\nexport function c(xs: any[]) { return xs.map(({ foo }) => foo()); }\nexport function d(xs: any[]) { xs.forEach(function (foo) { foo() }) }\nexport function e(list: any[]) { for (const foo of list) foo() }\nexport function f(list: any) { for (const foo in list) { foo() } }\nexport function g(list: any[]) { for (const [k, foo] of list) { foo() } }\nexport function h() { try { x() } catch (foo) { foo() } }\nexport function i() { try { x() } catch ({ foo }) { foo() } }\nexport function j(xs: any[]) { xs.map(async (foo = 1) => foo()) }\n",
        ),
    ]);
    let report = report(&repo, "foo", Some("m.ts"));

    for line in 2..=11 {
        assert_site(
            &report,
            "use.ts",
            line,
            SiteLabel::Likely,
            &binding_note(line),
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
