use code_kb_core::{
    ReferenceReport, SiteLabel, Workspace, find_julie_extract_binary, format_reference_report,
    get_symbol_by_id, open_read_only, reference_report, safe_tempdir, scan_workspace,
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

fn report_of(repo: &Repo, name: &str, path: &str, nth: usize) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND kind NOT IN ('export', 'import')
             ORDER BY start_line LIMIT 1 OFFSET ?3",
            rusqlite::params![name, path, nth as i64],
            |row| row.get(0),
        )
        .unwrap();
    let target = get_symbol_by_id(&conn, &id).unwrap().unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    report_of(repo, name, path, 0)
}

fn dump(report: &ReferenceReport) -> String {
    report
        .sites
        .iter()
        .map(|site| {
            format!(
                "{}:{} {} [{}] args={:?}",
                site.site.path,
                site.site.start_line.unwrap_or(0),
                site.label.as_str(),
                site.basis,
                site.arg_count
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn site(report: &ReferenceReport, path: &str, line: usize) -> (SiteLabel, String, Option<u32>) {
    let found = report
        .sites
        .iter()
        .find(|s| s.site.path == path && s.site.start_line == Some(line))
        .unwrap_or_else(|| panic!("no site at {path}:{line}\n{}", dump(report)));
    (found.label, found.basis.clone(), found.arg_count)
}

fn assert_excluded(report: &ReferenceReport, path: &str, line: usize, basis: &str) {
    let (label, found, _) = site(report, path, line);
    assert_eq!(
        (label, found.as_str()),
        (SiteLabel::Excluded, basis),
        "{path}:{line}\n{}",
        dump(report)
    );
}

fn assert_kept(report: &ReferenceReport, path: &str, line: usize) {
    let (label, basis, _) = site(report, path, line);
    assert_ne!(
        label,
        SiteLabel::Excluded,
        "{path}:{line} is excluded: {basis}\n{}",
        dump(report)
    );
}

const PY_FUNCTIONS: &[(&str, &str)] = &[
    ("pkg/a.py", "def render(x, y=1):\n    return x\n"),
    ("pkg/b.py", "def render(p):\n    return p\n"),
    (
        "pkg/use.py",
        "def go(obj, args):\n    obj.render(1, 2, 3)\n    obj.render(*args)\n    obj.render(1)\n    obj.render()\n    obj.render(1, y=2)\n",
    ),
];

#[test]
fn a_python_call_with_three_arguments_to_a_one_or_two_parameter_function_is_excluded() {
    let repo = scanned_repo(PY_FUNCTIONS);
    let report = report(&repo, "render", "pkg/a.py");
    assert_excluded(&report, "pkg/use.py", 2, "3 args; `render` takes 1-2");
    assert_eq!(site(&report, "pkg/use.py", 2).2, Some(3));
    assert_excluded(&report, "pkg/use.py", 5, "0 args; `render` takes 1-2");
    assert_kept(&report, "pkg/use.py", 4);
    assert_kept(&report, "pkg/use.py", 6);
}

#[test]
fn a_python_call_that_spreads_its_arguments_is_not_excluded_and_has_no_count() {
    let repo = scanned_repo(PY_FUNCTIONS);
    let report = report(&repo, "render", "pkg/a.py");
    assert_eq!(
        site(&report, "pkg/use.py", 3),
        (SiteLabel::Unresolved, "2 definitions".to_string(), None)
    );
}

const PY_METHODS: &[(&str, &str)] = &[
    (
        "pkg/base.py",
        "class Base:\n    def method(self, x):\n        return x\n\n    @staticmethod\n    def make(p, q):\n        return p\n\n    @classmethod\n    def build(cls, p):\n        return p\n\n    @unknown_decorator\n    def wrapped(self, p):\n        return p\n\n    def __init__(self, a, b=2):\n        self.a = a\n",
    ),
    (
        "pkg/other.py",
        "class Other:\n    def method(self, a, b, c, d):\n        return a\n\n    @staticmethod\n    def make(p):\n        return p\n\n    @classmethod\n    def build(cls, p, q, r):\n        return p\n\n    def wrapped(self, a, b, c, d):\n        return a\n\n    def __init__(self, a):\n        self.a = a\n",
    ),
    (
        "pkg/use.py",
        "def use(x, y):\n    x.method(1)\n    x.method(1, 2)\n    x.method(1, 2, 3)\n    y.make(1, 2)\n    y.make(1, 2, 3)\n    y.build(1)\n    y.build(1, 2)\n    y.wrapped(1, 2, 3)\n    y.wrapped(1)\n",
    ),
];

#[test]
fn a_python_method_accepts_the_receiver_as_its_first_argument() {
    let repo = scanned_repo(PY_METHODS);
    let report = report(&repo, "method", "pkg/base.py");
    assert_kept(&report, "pkg/use.py", 2);
    assert_kept(&report, "pkg/use.py", 3);
    assert_excluded(&report, "pkg/use.py", 4, "3 args; `method` takes 1");
}

#[test]
fn a_python_static_method_takes_every_parameter_and_a_class_method_drops_cls() {
    let repo = scanned_repo(PY_METHODS);
    let make = report(&repo, "make", "pkg/base.py");
    assert_kept(&make, "pkg/use.py", 5);
    assert_excluded(&make, "pkg/use.py", 6, "3 args; `make` takes 2");
    let build = report(&repo, "build", "pkg/base.py");
    assert_kept(&build, "pkg/use.py", 7);
    assert_excluded(&build, "pkg/use.py", 8, "2 args; `build` takes 1");
}

#[test]
fn a_python_target_with_a_decorator_that_can_change_its_signature_has_no_range() {
    let repo = scanned_repo(PY_METHODS);
    let report = report(&repo, "wrapped", "pkg/base.py");
    assert_kept(&report, "pkg/use.py", 9);
    assert_kept(&report, "pkg/use.py", 10);
}

const PY_VARIADIC: &[(&str, &str)] = &[
    (
        "pkg/lib.py",
        "def gather(*args, **kwargs):\n    return args\n\n\ndef head(first, *rest):\n    return first\n\n\ndef only(a, /, b, *, c, d=2):\n    return a\n",
    ),
    (
        "pkg/other.py",
        "def gather(a):\n    return a\n\n\ndef head(a, b, c):\n    return a\n\n\ndef only(a):\n    return a\n",
    ),
    (
        "pkg/use.py",
        "def use(m):\n    m.gather(1, 2, 3, 4)\n    m.gather()\n    m.head()\n    m.head(1, 2, 3, 4, 5)\n    m.only(1, 2)\n    m.only(1, 2, c=3)\n    m.only(1, 2, c=3, d=4)\n    m.only(1, 2, 3, 4, 5)\n",
    ),
];

#[test]
fn python_star_parameters_remove_the_upper_bound_and_keyword_only_markers_are_no_parameters() {
    let repo = scanned_repo(PY_VARIADIC);
    let gather = report(&repo, "gather", "pkg/lib.py");
    assert_kept(&gather, "pkg/use.py", 2);
    assert_kept(&gather, "pkg/use.py", 3);
    let head = report(&repo, "head", "pkg/lib.py");
    assert_excluded(&head, "pkg/use.py", 4, "0 args; `head` takes 1 or more");
    assert_kept(&head, "pkg/use.py", 5);
    let only = report(&repo, "only", "pkg/lib.py");
    assert_excluded(&only, "pkg/use.py", 6, "2 args; `only` takes 3-4");
    assert_kept(&only, "pkg/use.py", 7);
    assert_kept(&only, "pkg/use.py", 8);
    assert_excluded(&only, "pkg/use.py", 9, "5 args; `only` takes 3-4");
}

const PY_CONSTRUCTOR: &[(&str, &str)] = &[
    (
        "pkg/shapes.py",
        "class Circle:\n    def __init__(self, r, fill=None):\n        self.r = r\n",
    ),
    (
        "pkg/square.py",
        "class Circle:\n    def __init__(self, a, b, c):\n        self.a = a\n",
    ),
    (
        "pkg/child.py",
        "def build(helper, r):\n    helper.__init__(helper, r, None)\n    helper.__init__(helper, r, None, 1)\n",
    ),
    (
        "pkg/use.py",
        "def make():\n    Circle(1)\n    Circle(1, 2)\n    Circle(1, 2, 3, 4)\n    Circle()\n",
    ),
];

#[test]
fn python_class_calls_and_explicit_init_calls_use_the_range_of_init() {
    let repo = scanned_repo(PY_CONSTRUCTOR);
    let report = report(&repo, "__init__", "pkg/shapes.py");
    assert_kept(&report, "pkg/use.py", 2);
    assert_kept(&report, "pkg/use.py", 3);
    assert_excluded(&report, "pkg/use.py", 4, "4 args; `Circle` takes 1-2");
    assert_excluded(&report, "pkg/use.py", 5, "0 args; `Circle` takes 1-2");
    assert_kept(&report, "pkg/child.py", 2);
    assert_excluded(&report, "pkg/child.py", 3, "4 args; `__init__` takes 1-2");
}

const RUST_METHODS: &[(&str, &str)] = &[
    (
        "src/draw.rs",
        "pub struct Canvas;\n\nimpl Canvas {\n    pub fn paint(&self, a: i32) {}\n\n    pub fn stamp(a: i32) {}\n}\n",
    ),
    (
        "src/other.rs",
        "pub struct Sheet;\n\nimpl Sheet {\n    pub fn paint(&self, a: i32, b: i32, c: i32, d: i32) {}\n\n    pub fn stamp(a: i32, b: i32, c: i32) {}\n}\n",
    ),
    (
        "src/use.rs",
        "pub fn go() {\n    let x = source();\n    x.paint(1);\n    x.paint(1, 2);\n    x.paint(1, 2, 3);\n    x.stamp();\n    x.stamp(1);\n    x.stamp(1, 2);\n}\n",
    ),
];

#[test]
fn a_rust_method_accepts_the_receiver_as_its_first_argument_for_a_path_call() {
    let repo = scanned_repo(RUST_METHODS);
    let report = report(&repo, "paint", "src/draw.rs");
    assert_kept(&report, "src/use.rs", 3);
    assert_kept(&report, "src/use.rs", 4);
    assert_excluded(&report, "src/use.rs", 5, "3 args; `paint` takes 1");
}

#[test]
fn a_rust_associated_function_without_self_takes_exactly_its_parameters() {
    let repo = scanned_repo(RUST_METHODS);
    let report = report(&repo, "stamp", "src/draw.rs");
    assert_excluded(&report, "src/use.rs", 6, "0 args; `stamp` takes 1");
    assert_kept(&report, "src/use.rs", 7);
    assert_excluded(&report, "src/use.rs", 8, "2 args; `stamp` takes 1");
}

const CSHARP: &[(&str, &str)] = &[
    (
        "Ext.cs",
        "public static class Ext {\n    public static int Run(this string s, int a, int b = 2) { return 1; }\n    public int Opt(int a, int b = 1, int c = 2) { return 1; }\n    public int Many(int a, params int[] rest) { return 1; }\n}\n",
    ),
    (
        "Other.cs",
        "public class Other {\n    public int Run(int a, int b, int c, int d, int e) { return 1; }\n    public int Opt(int a, int b, int c, int d) { return 1; }\n    public int Many(int a, int b, int c) { return 1; }\n}\n",
    ),
    (
        "Use.cs",
        "public class Use {\n    public void Go() {\n        var x = Make();\n        x.Run(1);\n        x.Run(1, 2);\n        x.Run(1, 2, 3);\n        x.Run(1, 2, 3, 4);\n        x.Run();\n        x.Opt(b: 2, a: 1);\n        x.Opt();\n        x.Opt(1, 2, 3);\n        x.Many(1, 2, 3, 4, 5);\n        x.Many();\n    }\n}\n",
    ),
];

#[test]
fn a_csharp_extension_method_accepts_the_source_as_an_argument_and_has_optional_parameters() {
    let repo = scanned_repo(CSHARP);
    let report = report(&repo, "Run", "Ext.cs");
    assert_kept(&report, "Use.cs", 4);
    assert_kept(&report, "Use.cs", 5);
    assert_kept(&report, "Use.cs", 6);
    assert_excluded(&report, "Use.cs", 7, "4 args; `Run` takes 1-2");
    assert_excluded(&report, "Use.cs", 8, "0 args; `Run` takes 1-2");
}

#[test]
fn csharp_optional_named_and_params_parameters_widen_the_range() {
    let repo = scanned_repo(CSHARP);
    let opt = report(&repo, "Opt", "Ext.cs");
    assert_kept(&opt, "Use.cs", 9);
    assert_excluded(&opt, "Use.cs", 10, "0 args; `Opt` takes 1-3");
    assert_kept(&opt, "Use.cs", 11);
    let many = report(&repo, "Many", "Ext.cs");
    assert_kept(&many, "Use.cs", 12);
    assert_excluded(&many, "Use.cs", 13, "0 args; `Many` takes 1 or more");
}

const KOTLIN: &[(&str, &str)] = &[
    (
        "lib.kt",
        "package lib\n\nfun Mystery.each(n: Int, block: (Int) -> Unit) {}\n\nfun Mystery.pad(n: Int, fill: Char = ' ') {}\n\nfun Mystery.all(vararg xs: Int) {}\n",
    ),
    (
        "other.kt",
        "package other\n\nfun Gadget.each(a: Int, b: Int, c: Int, d: Int) {}\n\nfun Gadget.pad(a: Int, b: Int, c: Int, d: Int) {}\n\nfun Gadget.all(a: Int, b: Int) {}\n",
    ),
    (
        "use.kt",
        "package use\n\nfun go(s: Any) {\n    s.each(1) { }\n    s.each(1, 2, 3)\n    s.each()\n    s.pad(1)\n    s.pad(1, 'c', 3)\n    s.all()\n    s.all(1, 2, 3)\n}\n",
    ),
];

#[test]
fn kotlin_trailing_lambda_defaults_and_vararg_widen_the_range() {
    let repo = scanned_repo(KOTLIN);
    let each = report(&repo, "each", "lib.kt");
    assert_kept(&each, "use.kt", 4);
    assert_excluded(&each, "use.kt", 5, "3 args; `each` takes 2");
    assert_excluded(&each, "use.kt", 6, "0 args; `each` takes 2");
    let pad = report(&repo, "pad", "lib.kt");
    assert_kept(&pad, "use.kt", 7);
    assert_excluded(&pad, "use.kt", 8, "3 args; `pad` takes 1-2");
    let all = report(&repo, "all", "lib.kt");
    assert_kept(&all, "use.kt", 9);
    assert_kept(&all, "use.kt", 10);
}

const TYPESCRIPT: &[(&str, &str)] = &[
    (
        "src/lib.ts",
        "export function opt(a: number, b?: string, c = 1): void {}\nexport function rest(a: number, ...more: string[]): void {}\nexport function withThis(this: Window, a: number): void {}\nexport function over(a: number): void;\nexport function over(a: number, b: string): void;\nexport function over(a: number, b?: string): void {}\nexport function destructured({ x, y }: { x: number; y: number }, z = 3): void {}\n",
    ),
    (
        "src/other.ts",
        "export function opt(a: number, b: string, c: number, d: number, e: number): void {}\nexport function rest(a: number, b: number, c: number): void {}\nexport function withThis(a: number, b: number, c: number): void {}\nexport function over(a: number, b: number, c: number): void {}\nexport function destructured(a: number, b: number, c: number, d: number): void {}\n",
    ),
    (
        "src/use.ts",
        "export function go(m: any) {\n    m.opt();\n    m.opt(1);\n    m.opt(1, 'a', 2);\n    m.opt(1, 'a', 2, 3);\n    m.rest(1, 'a', 'b', 'c');\n    m.rest();\n    m.withThis(1);\n    m.withThis(1, 2);\n    m.over(1);\n    m.over(1, 'a');\n    m.over(1, 'a', 3);\n    m.destructured({ x: 1, y: 2 });\n    m.destructured({ x: 1, y: 2 }, 1, 2, 3, 4);\n}\n",
    ),
];

#[test]
fn typescript_optional_rest_this_and_overload_signatures_widen_the_range() {
    let repo = scanned_repo(TYPESCRIPT);
    let opt = report(&repo, "opt", "src/lib.ts");
    assert_excluded(&opt, "src/use.ts", 2, "0 args; `opt` takes 1-3");
    assert_kept(&opt, "src/use.ts", 3);
    assert_kept(&opt, "src/use.ts", 4);
    assert_excluded(&opt, "src/use.ts", 5, "4 args; `opt` takes 1-3");
    let rest = report(&repo, "rest", "src/lib.ts");
    assert_kept(&rest, "src/use.ts", 6);
    assert_excluded(&rest, "src/use.ts", 7, "0 args; `rest` takes 1 or more");
    let with_this = report(&repo, "withThis", "src/lib.ts");
    assert_kept(&with_this, "src/use.ts", 8);
    assert_excluded(&with_this, "src/use.ts", 9, "2 args; `withThis` takes 1");
    let over = report(&repo, "over", "src/lib.ts");
    assert_kept(&over, "src/use.ts", 10);
    assert_kept(&over, "src/use.ts", 11);
    assert_excluded(&over, "src/use.ts", 12, "3 args; `over` takes 1-2");
}

#[test]
fn a_typescript_destructured_parameter_leaves_the_target_without_a_range() {
    let repo = scanned_repo(TYPESCRIPT);
    let report = report(&repo, "destructured", "src/lib.ts");
    assert_kept(&report, "src/use.ts", 13);
    assert_kept(&report, "src/use.ts", 14);
}

const JAVASCRIPT: &[(&str, &str)] = &[
    ("src/lib.js", "export function opt(a, b) {}\n"),
    ("src/other.js", "export function opt(a, b, c, d) {}\n"),
    (
        "src/use.js",
        "export function go(m) {\n    m.opt();\n    m.opt(1, 2, 3);\n}\n",
    ),
];

#[test]
fn javascript_sites_never_use_the_arity_rule() {
    let repo = scanned_repo(JAVASCRIPT);
    let report = report(&repo, "opt", "src/lib.js");
    assert_kept(&report, "src/use.js", 2);
    assert_kept(&report, "src/use.js", 3);
}

const GO: &[(&str, &str)] = &[
    (
        "a/a.go",
        "package a\n\nfunc Pair(x int, y int) (int, int) { return x, y }\n\nfunc Take(a int, b int) {}\n\nfunc Many(a int, rest ...string) {}\n\ntype T struct{}\n\nfunc (t T) Do(a int) {}\n",
    ),
    (
        "b/b.go",
        "package b\n\nfunc Take(a int, b int, c int, d int) {}\n\nfunc Many(a int, b int, c int, d int) {}\n\ntype U struct{}\n\nfunc (u U) Do(a int, b int, c int, d int) {}\n",
    ),
    (
        "c/use.go",
        "package c\n\nfunc use() { m := mystery()\n\tm.Take(m.Pair(1, 2))\n\tm.Take(1)\n\tm.Take(1, 2, 3)\n\tm.Many()\n\tm.Many(1, \"a\", \"b\", \"c\")\n\tm.Many([]string{}...)\n\tm.Do(1, 2)\n\tm.Do(1, 2, 3)\n\tm.Do()\n}\n",
    ),
];

#[test]
fn go_multi_value_calls_variadic_parameters_and_method_expressions_are_not_excluded() {
    let repo = scanned_repo(GO);
    let take = report(&repo, "Take", "a/a.go");
    assert_kept(&take, "c/use.go", 4);
    assert_kept(&take, "c/use.go", 5);
    assert_excluded(&take, "c/use.go", 6, "3 args; `Take` takes 2");
    let many = report(&repo, "Many", "a/a.go");
    assert_excluded(&many, "c/use.go", 7, "0 args; `Many` takes 1 or more");
    assert_kept(&many, "c/use.go", 8);
    assert_kept(&many, "c/use.go", 9);
    let method = report(&repo, "Do", "a/a.go");
    assert_kept(&method, "c/use.go", 10);
    assert_excluded(&method, "c/use.go", 11, "3 args; `Do` takes 1");
    assert_excluded(&method, "c/use.go", 12, "0 args; `Do` takes 1");
}

const JAVA: &[(&str, &str)] = &[
    (
        "com/Shape.java",
        "package com;\n\npublic class Shape {\n    void area(int a) {}\n    void area(int a, int b) {}\n    void join(int a, String... rest) {}\n}\n",
    ),
    (
        "com/Other.java",
        "package com;\n\npublic class Other {\n    void area(int a, int b, int c, int d) {}\n    void join(int a, int b, int c) {}\n}\n",
    ),
    (
        "com/Use.java",
        "package com;\n\npublic class Use {\n    void go() {\n        var s = make();\n        s.area(1);\n        s.area(1, 2);\n        s.area(1, 2, 3);\n        s.area();\n        s.join(1);\n        s.join(1, \"a\", \"b\", \"c\");\n        s.join();\n    }\n}\n",
    ),
];

#[test]
fn a_java_overload_pair_accepts_both_counts_and_each_overload_its_own() {
    let repo = scanned_repo(JAVA);
    let report = report(&repo, "area", "com/Shape.java");
    assert_kept(&report, "com/Use.java", 6);
    assert_excluded(
        &report,
        "com/Use.java",
        7,
        "arguments `(int, int)` bind only `area(int, int)`",
    );
    let pair = report_of(&repo, "area", "com/Shape.java", 1);
    assert_excluded(
        &pair,
        "com/Use.java",
        6,
        "arguments `(int)` bind only `area(int)`",
    );
    assert_kept(&pair, "com/Use.java", 7);
    assert_excluded(&report, "com/Use.java", 8, "3 args; `area` takes 1-2");
    assert_excluded(&report, "com/Use.java", 9, "0 args; `area` takes 1-2");
}

#[test]
fn java_varargs_remove_the_upper_bound() {
    let repo = scanned_repo(JAVA);
    let report = report(&repo, "join", "com/Shape.java");
    assert_kept(&report, "com/Use.java", 10);
    assert_kept(&report, "com/Use.java", 11);
    assert_excluded(
        &report,
        "com/Use.java",
        12,
        "0 args; `join` takes 1 or more",
    );
}

const JAVA_CONSTRUCTORS: &[(&str, &str)] = &[
    (
        "com/Widget.java",
        "package com;\n\npublic class Widget {\n    public Widget(int x) {}\n    public Widget(int x, int y) {}\n\n    static Widget twin() {\n        return new Widget(1, 2);\n    }\n}\n",
    ),
    (
        "com/Point.java",
        "package com;\n\npublic record Point(int x, int y) {\n    public Point(int x) {\n        this(x, 0);\n    }\n}\n",
    ),
    (
        "com/Use.java",
        "package com;\n\npublic class Use {\n    void go() {\n        new Widget(1);\n        new Widget(1, 2);\n        new Widget(1, 2, 3);\n        new Point(1, 2);\n        new Point(1);\n    }\n}\n",
    ),
];

#[test]
fn a_java_class_keeps_every_new_expression_and_a_record_constructor_only_its_count() {
    let repo = scanned_repo(JAVA_CONSTRUCTORS);
    let widget = report(&repo, "Widget", "com/Widget.java");
    assert_kept(&widget, "com/Use.java", 5);
    assert_kept(&widget, "com/Use.java", 6);
    assert_kept(&widget, "com/Use.java", 7);
    let point = report_of(&repo, "Point", "com/Point.java", 1);
    let (label, _, _) = site(&point, "com/Use.java", 8);
    assert_eq!(label, SiteLabel::Excluded, "{}", dump(&point));
    assert_kept(&point, "com/Use.java", 9);
}

const KOTLIN_CONSTRUCTORS: &[(&str, &str)] = &[
    (
        "k/K.kt",
        "package k\n\nclass K(val x: Int, y: Int = 2) {\n    constructor(z: String) : this(1)\n}\n",
    ),
    (
        "k2/K.kt",
        "package k2\n\nclass K(val a: Int, val b: Int, val c: Int, val d: Int)\n",
    ),
    (
        "k/Use.kt",
        "package k\n\nfun go() {\n    K(1)\n    K(1, 2)\n    K(\"a\")\n    K(1, 2, 3, 4)\n}\n",
    ),
];

#[test]
fn a_kotlin_secondary_constructor_leaves_the_calls_of_the_primary_one_to_it() {
    let repo = scanned_repo(KOTLIN_CONSTRUCTORS);
    let report = report_of(&repo, "K", "k/K.kt", 1);
    assert_excluded(
        &report,
        "k/Use.kt",
        4,
        "arguments `(Int)` bind only `K(Int, Int)`",
    );
    assert_excluded(
        &report,
        "k/Use.kt",
        5,
        "arguments `(Int, Int)` bind only `K(Int, Int)`",
    );
    for line in [6, 7] {
        assert_kept(&report, "k/Use.kt", line);
    }
}

const NOT_CALLABLE: &[(&str, &str)] = &[
    (
        "a/a.go",
        "package a\n\nvar Handler func(int, int)\n\ntype Hooks struct {\n\tOnDone func()\n}\n",
    ),
    (
        "b/b.go",
        "package b\n\nfunc Handler(a int) {}\n\ntype Other struct{}\n\nfunc (o Other) OnDone(a int, b int, c int) {}\n",
    ),
    (
        "c/use.go",
        "package c\n\nfunc use() {\n\tm := build()\n\tm.Handler(1, 2, 3)\n\tm.Handler()\n\tm.OnDone(1)\n}\n",
    ),
];

#[test]
fn a_target_that_is_no_function_has_no_range() {
    let repo = scanned_repo(NOT_CALLABLE);
    let handler = report(&repo, "Handler", "a/a.go");
    assert_kept(&handler, "c/use.go", 5);
    assert_kept(&handler, "c/use.go", 6);
    let on_done = report(&repo, "OnDone", "a/a.go");
    assert_kept(&on_done, "c/use.go", 7);
}

#[test]
fn the_text_row_adds_the_argument_count_after_the_basis_without_repeating_an_exclusion_count() {
    let repo = scanned_repo(PY_FUNCTIONS);
    let report = report(&repo, "render", "pkg/a.py");
    let text = format_reference_report(&report, 20);
    assert!(
        text.contains("- `go` [pkg/use.py:4] (kind: calls; 2 definitions; 1 arg)"),
        "{text}"
    );
    assert!(
        text.contains("- `go` [pkg/use.py:2] (kind: calls; 3 args; `render` takes 1-2)"),
        "{text}"
    );
    assert!(
        text.contains("- `go` [pkg/use.py:3] (kind: calls; 2 definitions)"),
        "{text}"
    );
}

#[test]
fn the_json_site_carries_arg_count_only_when_known() {
    let repo = scanned_repo(PY_FUNCTIONS);
    let report = report(&repo, "render", "pkg/a.py");
    let json = serde_json::to_value(&report).unwrap();
    let sites = json["sites"].as_array().unwrap();
    let at = |line: u64| {
        sites
            .iter()
            .find(|site| site["start_line"] == line)
            .unwrap_or_else(|| panic!("no site at line {line}"))
    };
    assert_eq!(at(2)["arg_count"], 3);
    assert!(at(3).get("arg_count").is_none());
}

const PY_PATCHED: &[(&str, &str)] = &[
    ("pkg/lib.py", "def patched(self, a):\n    return a\n"),
    ("pkg/other.py", "def patched(a, b, c, d):\n    return a\n"),
    (
        "pkg/use.py",
        "def use(o):\n    o.patched(1)\n    o.patched(1, 2)\n    o.patched(1, 2, 3)\n",
    ),
];

#[test]
fn a_python_function_with_a_receiver_parameter_may_be_called_with_or_without_it() {
    let repo = scanned_repo(PY_PATCHED);
    let report = report(&repo, "patched", "pkg/lib.py");
    assert_kept(&report, "pkg/use.py", 2);
    assert_kept(&report, "pkg/use.py", 3);
    assert_excluded(&report, "pkg/use.py", 4, "3 args; `patched` takes 1");
}

#[test]
fn a_proven_site_keeps_its_label_whatever_its_argument_count() {
    let repo = scanned_repo(&[(
        "pkg/a.py",
        "def one(x):\n    return x\n\n\ndef caller():\n    return one(1, 2, 3)\n",
    )]);
    let report = report(&repo, "one", "pkg/a.py");
    assert_eq!(
        site(&report, "pkg/a.py", 6),
        (SiteLabel::Proven, "same file".to_string(), Some(3))
    );
}

const JAVA_LAMBDAS: &[(&str, &str)] = &[
    (
        "com/Task.java",
        "package com;\n\npublic class Task {\n    void run(int x, int y) {\n        Runnable r = () -> {};\n        java.util.function.IntBinaryOperator op = (a, b) -> a + b;\n        java.util.List.of(1).forEach(item -> System.out.println(item));\n    }\n}\n",
    ),
    (
        "com/Other.java",
        "package com;\n\npublic class Other {\n    void run(int a, int b, int c, int d) {}\n}\n",
    ),
    (
        "com/Use.java",
        "package com;\n\npublic class Use {\n    void go() {\n        var t = make();\n        t.run(1, 2);\n        t.run(1, 2, 3);\n        t.run(1);\n    }\n}\n",
    ),
];

#[test]
fn the_parameters_of_lambdas_in_a_body_are_no_parameters_of_the_method() {
    let repo = scanned_repo(JAVA_LAMBDAS);
    let report = report(&repo, "run", "com/Task.java");
    assert_kept(&report, "com/Use.java", 6);
    assert_excluded(&report, "com/Use.java", 7, "3 args; `run` takes 2");
    assert_excluded(&report, "com/Use.java", 8, "1 arg; `run` takes 2");
}

const CSHARP_CONSTRUCTORS: &[(&str, &str)] = &[
    (
        "A.cs",
        "public class A {\n    public A(int x, int y = 3) {}\n    public A() : this(1) {}\n}\npublic struct S {\n    public S(int x) {}\n}\n",
    ),
    (
        "Other.cs",
        "public class A {\n    public A(int a, int b, int c, int d) {}\n}\n\npublic class P {\n    public P(int a, int b, int c, int d, int e) {}\n}\n\npublic class R {\n    public R(int a, int b, int c, int d, int e, int f) {}\n}\n",
    ),
    (
        "Record.cs",
        "public record R(int A, int B) {\n    public R(int a) : this(a, 0) {}\n}\n",
    ),
    (
        "Primary.cs",
        "public class P(int x) {\n    public P(int x, int y) : this(x) {}\n}\n",
    ),
    (
        "Use.cs",
        "public class Use {\n    void Go() {\n        new A(1);\n        new A(1, 2);\n        new A(1, 2, 3);\n        new A(1, 2, 3, 4, 5);\n        new S();\n        new S(1, 2);\n        new P(1);\n        new P(1, 2);\n        new P(1, 2, 3);\n        new R(1);\n        new R(1, 2);\n        new R(1, 2, 3);\n    }\n}\n",
    ),
];

#[test]
fn a_csharp_constructor_group_decides_the_new_expressions_of_a_class() {
    let repo = scanned_repo(CSHARP_CONSTRUCTORS);
    let report = report_of(&repo, "A", "A.cs", 1);
    assert_kept(&report, "Use.cs", 3);
    assert_kept(&report, "Use.cs", 4);
    assert_excluded(&report, "Use.cs", 5, "3 args; `A` takes 0-2");
    assert_excluded(&report, "Use.cs", 6, "5 args; `A` takes 0-2");
}

#[test]
fn a_csharp_primary_constructor_with_parameters_outside_the_rows_takes_the_other_counts() {
    let repo = scanned_repo(CSHARP_CONSTRUCTORS);
    let report = report_of(&repo, "P", "Primary.cs", 1);
    assert_excluded(&report, "Use.cs", 9, "arguments `(int)` bind only `P(…)`");
    assert_kept(&report, "Use.cs", 10);
    assert_excluded(
        &report,
        "Use.cs",
        11,
        "arguments `(int, int, int)` bind only `P(…)`",
    );
}

#[test]
fn a_csharp_record_constructor_leaves_the_calls_of_the_canonical_constructor_to_it() {
    let repo = scanned_repo(CSHARP_CONSTRUCTORS);
    let report = report_of(&repo, "R", "Record.cs", 1);
    assert_kept(&report, "Use.cs", 12);
    assert_excluded(
        &report,
        "Use.cs",
        13,
        "arguments `(int, int)` bind only `R(int, int)`",
    );
    assert_kept(&report, "Use.cs", 14);
}

#[test]
fn a_csharp_struct_constructor_leaves_other_counts_to_the_implicit_default_constructor() {
    let repo = scanned_repo(CSHARP_CONSTRUCTORS);
    let report = report_of(&repo, "S", "A.cs", 1);
    assert_excluded(&report, "Use.cs", 7, "arguments `()` bind only `S(…)`");
    assert_excluded(
        &report,
        "Use.cs",
        8,
        "arguments `(int, int)` bind only `S(…)`",
    );
}

const TS_CONSTRUCTORS: &[(&str, &str)] = &[
    (
        "src/shape.ts",
        "export class Shape {\n    constructor(public w: number, h?: number) {}\n}\n",
    ),
    (
        "src/other.ts",
        "export class Shape {\n    constructor(a: number, b: number, c: number, d: number) {}\n}\n",
    ),
    (
        "src/use.ts",
        "export function go() {\n    new Shape(1);\n    new Shape(1, 2);\n    new Shape(1, 2, 3);\n    new Shape();\n}\n",
    ),
];

#[test]
fn a_typescript_constructor_decides_the_new_expressions_of_its_class() {
    let repo = scanned_repo(TS_CONSTRUCTORS);
    let report = report(&repo, "constructor", "src/shape.ts");
    assert_kept(&report, "src/use.ts", 2);
    assert_kept(&report, "src/use.ts", 3);
    assert_excluded(&report, "src/use.ts", 4, "3 args; `Shape` takes 1-2");
    assert_excluded(&report, "src/use.ts", 5, "0 args; `Shape` takes 1-2");
}

const TS_PROPERTY: &[(&str, &str)] = &[
    (
        "src/hooks.ts",
        "export class Hooks {\n    onDone: (a: number) => void = () => {};\n}\n",
    ),
    (
        "src/other.ts",
        "export class Other {\n    onDone(a: number, b: number, c: number) {}\n}\n",
    ),
    (
        "src/use.ts",
        "export function go() {\n    const m = build();\n    m.onDone(1);\n    m.onDone(1, 2, 3, 4);\n}\n",
    ),
];

#[test]
fn a_typescript_property_of_a_function_type_has_no_range() {
    let repo = scanned_repo(TS_PROPERTY);
    let report = report(&repo, "onDone", "src/hooks.ts");
    assert_kept(&report, "src/use.ts", 3);
    assert_kept(&report, "src/use.ts", 4);
}

const CSHARP_THIS_FORMS: &[(&str, &str)] = &[
    (
        "Ext.cs",
        "public static class Ext {\n    public static int Tag([NotNull] this Lib l, int a) { return a; }\n    public static int Wide([NotNull][AllowNull] ref this Big b, int a, int c = 1) { return a; }\n    public static int Late(ref readonly this Big b, int a) { return a; }\n}\n",
    ),
    ("Lib.cs", "public class Lib {}\npublic struct Big {}\n"),
    (
        "Other.cs",
        "public class Other {\n    public int Tag(int a, int b, int c, int d) { return 1; }\n    public int Wide(int a, int b, int c, int d, int e) { return 1; }\n    public int Late(int a, int b, int c, int d, int e) { return 1; }\n}\n",
    ),
    (
        "Use.cs",
        "public class Use {\n    public void Go() {\n        var x = Make();\n        x.Tag(1);\n        x.Tag(1, 2);\n        x.Tag(1, 2, 3);\n        x.Tag();\n        x.Wide(1);\n        x.Wide(1, 2, 3);\n        x.Wide(1, 2, 3, 4);\n        x.Late(1);\n        x.Late(1, 2, 3);\n        Lib l = new Lib();\n        l.Tag(1);\n    }\n}\n",
    ),
];

#[test]
fn a_csharp_extension_method_with_attributes_or_ref_modifiers_before_this_takes_the_source_as_an_argument()
 {
    let repo = scanned_repo(CSHARP_THIS_FORMS);
    let tag = report(&repo, "Tag", "Ext.cs");
    assert_kept(&tag, "Use.cs", 4);
    assert_kept(&tag, "Use.cs", 5);
    assert_excluded(&tag, "Use.cs", 6, "3 args; `Tag` takes 1");
    assert_excluded(&tag, "Use.cs", 7, "0 args; `Tag` takes 1");
    let wide = report(&repo, "Wide", "Ext.cs");
    assert_kept(&wide, "Use.cs", 8);
    assert_kept(&wide, "Use.cs", 9);
    assert_excluded(&wide, "Use.cs", 10, "4 args; `Wide` takes 1-2");
    let late = report(&repo, "Late", "Ext.cs");
    assert_kept(&late, "Use.cs", 11);
    assert_excluded(&late, "Use.cs", 12, "3 args; `Late` takes 1");
}

#[test]
fn a_receiver_of_the_extended_type_does_not_exclude_an_extension_method_with_an_attribute_before_this()
 {
    let repo = scanned_repo(CSHARP_THIS_FORMS);
    let tag = report(&repo, "Tag", "Ext.cs");
    assert_kept(&tag, "Use.cs", 14);
}

const CSHARP_THIS_MODIFIERS: &[(&str, &str)] = &[
    (
        "Ext.cs",
        "public struct Big { public int V; }\n\npublic static class Ext {\n    public static int ExtRef(this ref int x, int k) { return k; }\n    public static int ExtIn(this in Big b, int k) { return k; }\n    public static int ExtScoped(this scoped ref Big b, int k) { return k; }\n    public static int ExtRefReadonly(this ref readonly Big b, int k) { return k; }\n}\n",
    ),
    (
        "Other.cs",
        "public class Other {\n    public int ExtRef(int a, int b, int c) { return a; }\n    public int ExtIn(int a, int b, int c) { return a; }\n    public int ExtScoped(int a, int b, int c) { return a; }\n    public int ExtRefReadonly(int a, int b, int c) { return a; }\n}\n",
    ),
    (
        "Use.cs",
        "public static class Use {\n    public static void Go() {\n        int a = 1;\n        Big big = new Big();\n        a.ExtRef(2);\n        big.ExtIn(1);\n        big.ExtScoped(2);\n        big.ExtRefReadonly(3);\n    }\n}\n",
    ),
];

#[test]
fn a_receiver_of_the_extended_type_does_not_exclude_an_extension_method_with_modifiers_after_this()
{
    let repo = scanned_repo(CSHARP_THIS_MODIFIERS);
    for (name, line) in [
        ("ExtRef", 5),
        ("ExtIn", 6),
        ("ExtScoped", 7),
        ("ExtRefReadonly", 8),
    ] {
        let report = report(&repo, name, "Ext.cs");
        assert_kept(&report, "Use.cs", line);
    }
}

const CSHARP_OVERRIDES: &[(&str, &str)] = &[
    (
        "IGreeter.cs",
        "public interface IGreeter {\n    string Greet(string name, bool loud = false);\n}\n",
    ),
    (
        "Impl.cs",
        "public class Impl : IGreeter {\n    public string Greet(string name, bool loud) { return name; }\n}\n",
    ),
    (
        "Other.cs",
        "public class Other {\n    public string Greet(int a, int b, int c, int d) { return \"\"; }\n}\n",
    ),
    (
        "Use.cs",
        "public class Use {\n    public void Go() {\n        var g = Make();\n        g.Greet(\"x\");\n        g.Greet(\"x\", true);\n        g.Greet(\"x\", true, 1);\n        g.Greet();\n    }\n}\n",
    ),
];

#[test]
fn a_csharp_override_accepts_the_default_values_of_the_interface_method_it_implements() {
    let repo = scanned_repo(CSHARP_OVERRIDES);
    let report = report(&repo, "Greet", "Impl.cs");
    assert_kept(&report, "Use.cs", 4);
    assert_kept(&report, "Use.cs", 5);
    assert_excluded(&report, "Use.cs", 6, "3 args; `Greet` takes 1-2");
    assert_excluded(&report, "Use.cs", 7, "0 args; `Greet` takes 1-2");
}

const KOTLIN_OVERRIDES: &[(&str, &str)] = &[
    (
        "lib/Greeter.kt",
        "package lib\n\ninterface Greeter {\n    fun greet(name: String, loud: Boolean = false): String\n}\n",
    ),
    (
        "lib/Impl.kt",
        "package lib\n\nclass Impl : Greeter {\n    override fun greet(name: String, loud: Boolean): String = name\n}\n",
    ),
    (
        "lib/Use.kt",
        "package lib\n\nfun go() {\n    val g = make()\n    g.greet(\"x\")\n    g.greet(\"x\", true)\n    g.greet(\"x\", true, 1)\n    g.greet()\n}\n",
    ),
];

#[test]
fn a_kotlin_override_inherits_the_default_values_of_the_method_it_overrides() {
    let repo = scanned_repo(KOTLIN_OVERRIDES);
    let report = report(&repo, "greet", "lib/Impl.kt");
    assert_kept(&report, "lib/Use.kt", 5);
    assert_kept(&report, "lib/Use.kt", 6);
    assert_excluded(&report, "lib/Use.kt", 7, "3 args; `greet` takes 1-2");
    assert_excluded(&report, "lib/Use.kt", 8, "0 args; `greet` takes 1-2");
}

const TS_OVERRIDES: &[(&str, &str)] = &[
    (
        "src/handler.ts",
        "export class Handler {\n    handle(req: string, res: number): void {}\n}\n",
    ),
    (
        "src/impl.ts",
        "import { Handler } from './handler';\n\nexport class Impl extends Handler {\n    handle(req: string): void {}\n}\n",
    ),
    (
        "src/use.ts",
        "export function go() {\n    const h = make();\n    h.handle('r', 1);\n    h.handle('r');\n    h.handle('r', 1, 2);\n    h.handle();\n}\n",
    ),
];

#[test]
fn a_typescript_override_with_fewer_parameters_accepts_the_arguments_of_its_base_method() {
    let repo = scanned_repo(TS_OVERRIDES);
    let report = report(&repo, "handle", "src/impl.ts");
    assert_kept(&report, "src/use.ts", 3);
    assert_kept(&report, "src/use.ts", 4);
    assert_excluded(&report, "src/use.ts", 5, "3 args; `handle` takes 1-2");
    assert_excluded(&report, "src/use.ts", 6, "0 args; `handle` takes 1-2");
}

#[test]
fn a_typescript_override_of_an_abstract_or_interface_method_has_no_range() {
    let repo = scanned_repo(&[
        (
            "src/handler.ts",
            "export abstract class Handler {\n    abstract handle(req: string, res: number): void;\n}\n\nexport interface Cb {\n    run(a: number, b: number): void;\n}\n",
        ),
        (
            "src/impl.ts",
            "import { Handler, Cb } from './handler';\n\nexport class Impl extends Handler {\n    handle(req: string): void {}\n}\n\nexport class CbImpl implements Cb {\n    run(a: number): void {}\n}\n",
        ),
        (
            "src/use.ts",
            "export function go() {\n    const h = make();\n    h.handle('r', 1, 2);\n    h.handle();\n    h.run(1, 2, 3);\n    h.run();\n}\n",
        ),
    ]);
    let handle = report(&repo, "handle", "src/impl.ts");
    assert_kept(&handle, "src/use.ts", 3);
    assert_kept(&handle, "src/use.ts", 4);
    let run = report(&repo, "run", "src/impl.ts");
    assert_kept(&run, "src/use.ts", 5);
    assert_kept(&run, "src/use.ts", 6);
}

#[test]
fn a_base_method_keeps_its_own_range_when_an_override_takes_another_count() {
    let repo = scanned_repo(&[
        (
            "src/base.ts",
            "export class Base {\n    handle(req: string): void {}\n}\n",
        ),
        (
            "src/impl.ts",
            "import { Base } from './base';\n\nexport class Impl extends Base {\n    handle(req: string, res?: number): void {}\n}\n",
        ),
        (
            "src/use.ts",
            "export function go() {\n    const h = make();\n    h.handle('r');\n    h.handle('r', 1);\n}\n",
        ),
    ]);
    let report = report(&repo, "handle", "src/base.ts");
    assert_kept(&report, "src/use.ts", 3);
    assert_excluded(&report, "src/use.ts", 4, "2 args; `handle` takes 1");
}

#[test]
fn a_typescript_override_of_a_method_without_a_range_has_no_range() {
    let repo = scanned_repo(&[
        (
            "src/base.ts",
            "export class Base {\n    handle({ a, b }: { a: number; b: number }, c = 1): void {}\n}\n",
        ),
        (
            "src/impl.ts",
            "import { Base } from './base';\n\nexport class Impl extends Base {\n    handle(req: string): void {}\n}\n",
        ),
        (
            "src/use.ts",
            "export function go() {\n    const h = make();\n    h.handle(1, 2, 3, 4);\n    h.handle();\n}\n",
        ),
    ]);
    let report = report(&repo, "handle", "src/impl.ts");
    assert_kept(&report, "src/use.ts", 3);
    assert_kept(&report, "src/use.ts", 4);
}

#[test]
fn a_typescript_override_of_a_base_imported_under_another_name_accepts_the_base_arguments() {
    let repo = scanned_repo(&[
        (
            "src/base.ts",
            "export class Base {\n    handle(req: string, res: number): void {}\n}\n",
        ),
        (
            "src/impl.ts",
            "import { Base as B } from './base';\n\nexport class Impl extends B {\n    handle(req: string): void {}\n}\n",
        ),
        (
            "src/use.ts",
            "export function go() {\n    const h = make();\n    h.handle('r', 1);\n    h.handle('r');\n    h.handle('r', 1, 2);\n    h.handle();\n}\n",
        ),
    ]);
    let report = report(&repo, "handle", "src/impl.ts");
    assert_kept(&report, "src/use.ts", 3);
    assert_kept(&report, "src/use.ts", 4);
    assert_excluded(&report, "src/use.ts", 5, "3 args; `handle` takes 1-2");
    assert_excluded(&report, "src/use.ts", 6, "0 args; `handle` takes 1-2");
    assert_eq!(
        report
            .overridden
            .iter()
            .map(|row| row.owner.as_str())
            .collect::<Vec<_>>(),
        ["Base"]
    );
}

#[test]
fn a_csharp_override_of_a_base_named_by_a_using_alias_accepts_the_base_arguments() {
    let repo = scanned_repo(&[
        (
            "Lib.cs",
            "using B = N.Base1;\n\nnamespace N {\n    public class Base1 {\n        public virtual int F(int a, int b = 0) { return a; }\n    }\n}\n\npublic class A3 : B {\n    public override int F(int a, int b) { return a + b; }\n}\n",
        ),
        (
            "Use.cs",
            "public class Use {\n    public void Go() {\n        var b = Make();\n        b.F(1);\n        b.F(1, 2);\n        b.F(1, 2, 3);\n        b.F();\n    }\n}\n",
        ),
    ]);
    let report = report(&repo, "F", "Lib.cs");
    let derived = report_of(&repo, "F", "Lib.cs", 1);
    assert_kept(&derived, "Use.cs", 4);
    assert_kept(&derived, "Use.cs", 5);
    assert_excluded(&derived, "Use.cs", 6, "3 args; `F` takes 1-2");
    assert_excluded(&derived, "Use.cs", 7, "0 args; `F` takes 1-2");
    assert_kept(&report, "Use.cs", 4);
}

#[test]
fn a_kotlin_override_of_a_base_imported_under_another_name_accepts_the_base_arguments() {
    let repo = scanned_repo(&[
        (
            "lib/Base.kt",
            "package lib\n\nopen class Base {\n    open fun handle(a: Int, b: Int = 0): Int = a\n}\n",
        ),
        (
            "app/Impl.kt",
            "package app\n\nimport lib.Base as B\n\nclass Impl : B() {\n    override fun handle(a: Int, b: Int): Int = a + b\n}\n",
        ),
        (
            "app/Use.kt",
            "package app\n\nfun go() {\n    val h = make()\n    h.handle(1)\n    h.handle(1, 2)\n    h.handle(1, 2, 3)\n    h.handle()\n}\n",
        ),
    ]);
    let report = report(&repo, "handle", "app/Impl.kt");
    assert_kept(&report, "app/Use.kt", 5);
    assert_kept(&report, "app/Use.kt", 6);
    assert_excluded(&report, "app/Use.kt", 7, "3 args; `handle` takes 1-2");
    assert_excluded(&report, "app/Use.kt", 8, "0 args; `handle` takes 1-2");
}

#[test]
fn a_python_override_of_a_base_imported_under_another_name_accepts_the_base_arguments() {
    let repo = scanned_repo(&[
        (
            "base.py",
            "class Base:\n    def handle(self, req, res=0):\n        return req\n",
        ),
        (
            "impl.py",
            "from base import Base as B\n\n\nclass Impl(B):\n    def handle(self, req):\n        return req\n",
        ),
        (
            "use.py",
            "def go(h):\n    h.handle(1, 2)\n    h.handle(1)\n    h.handle(1, 2, 3, 4)\n    h.handle()\n",
        ),
    ]);
    let report = report(&repo, "handle", "impl.py");
    assert_kept(&report, "use.py", 2);
    assert_kept(&report, "use.py", 3);
    assert_excluded(&report, "use.py", 4, "4 args; `handle` takes 1-2");
    assert_excluded(&report, "use.py", 5, "0 args; `handle` takes 1-2");
}

#[test]
fn an_override_whose_ancestor_walk_stops_at_the_depth_limit_has_no_range() {
    let mut chain = String::from("export class C0 {\n    m(a: number, b: number): void {}\n}\n");
    for depth in 1..9 {
        chain.push_str(&format!(
            "export class C{depth} extends C{} {{}}\n",
            depth - 1
        ));
    }
    chain.push_str("export class C9 extends C8 {\n    m(a: number): void {}\n}\n");
    let use_file = "export function go() {\n    const h = make();\n    h.m(1, 2);\n    h.m();\n}\n";
    let repo = scanned_repo(&[("src/chain.ts", &chain), ("src/use.ts", use_file)]);
    let report = report(&repo, "m", "src/chain.ts");
    let nine = report_of(&repo, "m", "src/chain.ts", 1);
    assert_kept(&nine, "src/use.ts", 3);
    assert_kept(&nine, "src/use.ts", 4);
    assert_excluded(&report, "src/use.ts", 4, "0 args; `m` takes 2");
}

const JAVA_OVERRIDES: &[(&str, &str)] = &[
    (
        "com/Base.java",
        "package com;\n\npublic class Base {\n    public String join(String... parts) { return \"\"; }\n}\n",
    ),
    (
        "com/Impl.java",
        "package com;\n\npublic class Impl extends Base {\n    @Override\n    public String join(String[] parts) { return \"\"; }\n}\n",
    ),
    (
        "com/Use.java",
        "package com;\n\npublic class Use {\n    void go() {\n        var b = make();\n        b.join(\"a\", \"b\", \"c\");\n        b.join();\n        b.join(\"a\");\n    }\n}\n",
    ),
];

#[test]
fn a_java_array_parameter_that_overrides_a_varargs_parameter_accepts_any_count() {
    let repo = scanned_repo(JAVA_OVERRIDES);
    let report = report(&repo, "join", "com/Impl.java");
    assert_kept(&report, "com/Use.java", 6);
    assert_kept(&report, "com/Use.java", 7);
    assert_kept(&report, "com/Use.java", 8);
}

const TS_VOID: &[(&str, &str)] = &[
    (
        "src/lib.ts",
        "export function voidArg(x: void): number {\n    return 1;\n}\n\nexport function unionVoid(x: number | void): number {\n    return 1;\n}\n\nexport function callback(x: () => void): number {\n    return 1;\n}\n\nexport class Vd {\n    m3(x: void): number {\n        return 1;\n    }\n}\n",
    ),
    (
        "src/other.ts",
        "export function voidArg(a: number, b: number, c: number, d: number): void {}\nexport function unionVoid(a: number, b: number, c: number, d: number): void {}\nexport function callback(a: number, b: number, c: number, d: number): void {}\nexport class Other {\n    m3(a: number, b: number, c: number, d: number): void {}\n}\n",
    ),
    (
        "src/use.ts",
        "export function go(m: any) {\n    m.voidArg();\n    m.voidArg(1, 2);\n    m.unionVoid();\n    m.callback();\n}\n\nexport function member() {\n    const v = make();\n    v.m3();\n}\n",
    ),
];

#[test]
fn a_typescript_parameter_typed_void_or_a_union_with_void_can_be_left_out() {
    let repo = scanned_repo(TS_VOID);
    let void_arg = report(&repo, "voidArg", "src/lib.ts");
    assert_kept(&void_arg, "src/use.ts", 2);
    assert_excluded(&void_arg, "src/use.ts", 3, "2 args; `voidArg` takes 0-1");
    assert_kept(&report(&repo, "unionVoid", "src/lib.ts"), "src/use.ts", 4);
    assert_kept(&report(&repo, "m3", "src/lib.ts"), "src/use.ts", 10);
}

#[test]
fn a_typescript_parameter_of_a_function_type_that_returns_void_is_required() {
    let repo = scanned_repo(TS_VOID);
    let callback = report(&repo, "callback", "src/lib.ts");
    assert_excluded(&callback, "src/use.ts", 5, "0 args; `callback` takes 1");
}

const PY_CLASS_ATTRIBUTES: &[(&str, &str)] = &[
    (
        "pkg/lib.py",
        "def render(view, name):\n    return name\n\n\ndef plain(a, b):\n    return a\n\n\ndef wrapped(a, b):\n    return a\n\n\nclass View:\n    render = render\n    helper = staticmethod(plain)\n\n\nclass Holder:\n    wrapped = staticmethod(wrapped)\n",
    ),
    (
        "pkg/use.py",
        "def go(v):\n    v.render('t')\n    v.render('t', 1)\n    v.render()\n    v.plain(1)\n    v.wrapped(1, 2, 3)\n",
    ),
];

#[test]
fn a_python_function_bound_as_a_class_attribute_of_its_name_has_no_range() {
    let repo = scanned_repo(PY_CLASS_ATTRIBUTES);
    let render = report(&repo, "render", "pkg/lib.py");
    for line in [2, 3, 4] {
        assert_kept(&render, "pkg/use.py", line);
    }
    let wrapped = report(&repo, "wrapped", "pkg/lib.py");
    assert_kept(&wrapped, "pkg/use.py", 6);
}

#[test]
fn a_python_function_that_a_class_attribute_of_another_name_holds_keeps_its_range() {
    let repo = scanned_repo(PY_CLASS_ATTRIBUTES);
    let plain = report(&repo, "plain", "pkg/lib.py");
    assert_excluded(&plain, "pkg/use.py", 5, "1 arg; `plain` takes 2");
}

const GO_GROUPED: &[(&str, &str)] = &[
    (
        "a.go",
        "package main\n\ntype T struct{}\n\nfunc Pair(a, b int) {}\nfunc Triple(a, b, c int) {}\nfunc Mixed(a, b int, c string) {}\nfunc (t T) M2(a, b int) int { return a }\nfunc (t T) M4(w, x, y, z int) int { return w }\nfunc Group(a, b int, c string) {}\n",
    ),
    (
        "use.go",
        "package main\n\nfunc use() { m := mystery()\n\tm.Pair(1, 2, 3)\n\tm.Triple(1, 2, 3, 4)\n\tm.Mixed(1, 2, 3, 4)\n\tm.M2(1, 2, 3, 4)\n\tm.M4(1, 2, 3, 4, 5, 6)\n\tm.Group(1, 2, 3, 4)\n\tm.Pair(1, 2)\n\tm.M2(1, 2)\n}\n",
    ),
];

#[test]
fn go_parameters_grouped_under_one_type_count_one_parameter_each() {
    let repo = scanned_repo(GO_GROUPED);
    let excluded = [
        ("Pair", 4, "3 args; `Pair` takes 2"),
        ("Triple", 5, "4 args; `Triple` takes 3"),
        ("Mixed", 6, "4 args; `Mixed` takes 3"),
        ("M2", 7, "4 args; `M2` takes 2"),
        ("M4", 8, "6 args; `M4` takes 4"),
        ("Group", 9, "4 args; `Group` takes 3"),
    ];
    for (name, line, basis) in excluded {
        assert_excluded(&report(&repo, name, "a.go"), "use.go", line, basis);
    }
    assert_kept(&report(&repo, "Pair", "a.go"), "use.go", 10);
    assert_kept(&report(&repo, "M2", "a.go"), "use.go", 11);
}

fn wide_class(interface_count: usize) -> String {
    let interfaces: String = (0..interface_count)
        .map(|n| format!("public interface I{n} {{}}\n"))
        .collect();
    let bases = (0..interface_count)
        .map(|n| format!("I{n}"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{interfaces}public class Wide : {bases} {{\n    public int Run(int a) {{ return a; }}\n}}\n"
    )
}

#[test]
fn an_override_whose_ancestor_walk_stops_at_the_type_limit_has_no_range() {
    let use_file = "public class Use {\n    public void Go() {\n        var w = Make();\n        w.Run(1, 2);\n    }\n}\n";
    let few = wide_class(3);
    let repo = scanned_repo(&[("Wide.cs", &few), ("Use.cs", use_file)]);
    assert_excluded(
        &report(&repo, "Run", "Wide.cs"),
        "Use.cs",
        4,
        "2 args; `Run` takes 1",
    );
    let many = wide_class(51);
    let repo = scanned_repo(&[("Wide.cs", &many), ("Use.cs", use_file)]);
    assert_kept(&report(&repo, "Run", "Wide.cs"), "Use.cs", 4);
}
