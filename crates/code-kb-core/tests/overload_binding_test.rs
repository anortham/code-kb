use code_kb_core::{
    ReferenceReport, SiteLabel, Workspace, edit_check_op, find_julie_extract_binary,
    format_blast_radius, format_edit_check, get_symbol_by_id, open_read_only, reference_report,
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

fn git(root: &Path, args: &[&str]) {
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
}

fn write(root: &Path, path: &str, content: &str) {
    let full = root.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, content).unwrap();
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
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

fn symbol_id(repo: &Repo, name: &str, path: &str, nth: usize) -> String {
    let conn = open_read_only(&repo.db).unwrap();
    conn.query_row(
        "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2
           AND kind IN ('function', 'method', 'constructor')
         ORDER BY start_line LIMIT 1 OFFSET ?3",
        rusqlite::params![name, path, nth as i64],
        |row| row.get(0),
    )
    .unwrap_or_else(|error| panic!("{name} #{nth} in {path}: {error}"))
}

fn report_of(repo: &Repo, name: &str, path: &str, nth: usize) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = get_symbol_by_id(&conn, &symbol_id(repo, name, path, nth))
        .unwrap()
        .unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn dump(report: &ReferenceReport) -> String {
    report
        .sites
        .iter()
        .map(|site| {
            format!(
                "{}:{} {} [{}] {}",
                site.site.path,
                site.site.start_line.unwrap_or(0),
                site.label.as_str(),
                site.basis,
                site.site.kind
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_of(repo: &Repo, path: &str, needle: &str) -> usize {
    let text = fs::read_to_string(repo.root.join(path)).unwrap();
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("{needle} not in {path}"))
        + 1
}

fn site(report: &ReferenceReport, path: &str, line: usize) -> (SiteLabel, String) {
    let found = report
        .sites
        .iter()
        .find(|s| s.site.path == path && s.site.start_line == Some(line))
        .unwrap_or_else(|| panic!("no site at {path}:{line}\n{}", dump(report)));
    (found.label, found.basis.clone())
}

fn assert_excluded(report: &ReferenceReport, path: &str, line: usize, basis: &str) {
    assert_eq!(
        site(report, path, line),
        (SiteLabel::Excluded, basis.to_string()),
        "{path}:{line}\n{}",
        dump(report)
    );
}

fn assert_kept(report: &ReferenceReport, path: &str, line: usize) {
    let (label, basis) = site(report, path, line);
    assert_ne!(
        label,
        SiteLabel::Excluded,
        "{path}:{line} is excluded: {basis}\n{}",
        dump(report)
    );
}

fn assert_not_bound_elsewhere(report: &ReferenceReport, path: &str, line: usize) {
    let (_, basis) = site(report, path, line);
    assert!(
        !basis.starts_with("arguments") && !basis.contains(" fit"),
        "{path}:{line}\n{}",
        dump(report)
    );
}

fn impacted(repo: &Repo, id: &str) -> Vec<String> {
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let result = code_kb_core::blast_radius_selected_op(
        &workspace,
        Some(&repo.db),
        &conn,
        Some(code_kb_core::SymbolSelector::Id(id.to_string())),
        None,
        2,
        20,
    )
    .unwrap();
    let mut callers: Vec<String> = result
        .impacted_symbols
        .iter()
        .map(|symbol| symbol.name.to_lowercase())
        .collect();
    callers.sort();
    callers.dedup();
    assert!(
        !callers.is_empty() || result.likely_tests.is_empty(),
        "{}",
        format_blast_radius(&result)
    );
    callers
}

fn check_text(repo: &Repo) -> String {
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    format_edit_check(&edit_check_op(&workspace, &repo.db, &conn, None).expect("edit check"))
}

const JAVA_RUN: &str = "package p;\n\nclass Base {}\nclass Sub extends Base {}\nclass Other {}\n\nclass Ov {\n    void run(int a) {}\n    void run(String a) {}\n    void take(Base b) {}\n    void take(Other o) {}\n    <T> void pick(T x) {}\n    void pick(String s) {}\n}\n";
const JAVA_USE: &str = "package p;\n\nclass Use {\n    void go(Ov ov, Integer boxed, Object o, long wide, Sub sub) {\n        ov.run(1);\n        ov.run(\"x\");\n        ov.run(boxed);\n        ov.run(o);\n        ov.run(null);\n        ov.run(wide);\n        ov.take(sub);\n        ov.pick(1);\n        ov.pick(\"s\");\n        ov.pick(o);\n    }\n}\n";

#[test]
fn a_java_call_whose_argument_types_bind_another_overload_is_excluded() {
    let repo = scanned_repo(&[("src/p/Ov.java", JAVA_RUN), ("src/p/Use.java", JAVA_USE)]);
    let path = "src/p/Use.java";
    let line = |needle| line_of(&repo, path, needle);
    let run_int = report_of(&repo, "run", "src/p/Ov.java", 0);
    let run_string = report_of(&repo, "run", "src/p/Ov.java", 1);

    assert_kept(&run_int, path, line("ov.run(1)"));
    assert_excluded(
        &run_string,
        path,
        line("ov.run(1)"),
        "arguments `(int)` bind only `run(int)`",
    );
    assert_excluded(
        &run_int,
        path,
        line("ov.run(\"x\")"),
        "arguments `(String)` bind only `run(String)`",
    );
    assert_kept(&run_string, path, line("ov.run(\"x\")"));
    assert_kept(&run_int, path, line("ov.run(boxed)"));
    assert_excluded(
        &run_string,
        path,
        line("ov.run(boxed)"),
        "arguments `(Integer)` bind only `run(int)`",
    );
    for undecided in ["ov.run(o)", "ov.run(null)"] {
        assert_kept(&run_int, path, line(undecided));
        assert_kept(&run_string, path, line(undecided));
    }
    assert_kept(&run_int, path, line("ov.run(wide)"));
    assert_kept(&run_string, path, line("ov.run(wide)"));

    let take_base = report_of(&repo, "take", "src/p/Ov.java", 0);
    let take_other = report_of(&repo, "take", "src/p/Ov.java", 1);
    assert_kept(&take_base, path, line("ov.take(sub)"));
    assert_kept(&take_other, path, line("ov.take(sub)"));

    let pick_generic = report_of(&repo, "pick", "src/p/Ov.java", 0);
    let pick_string = report_of(&repo, "pick", "src/p/Ov.java", 1);
    assert_kept(&pick_generic, path, line("ov.pick(1)"));
    assert_excluded(
        &pick_string,
        path,
        line("ov.pick(1)"),
        "arguments `(int)` bind only `pick(T)`",
    );
    assert_excluded(
        &pick_generic,
        path,
        line("ov.pick(\"s\")"),
        "arguments `(String)` bind only `pick(String)`",
    );
    assert_kept(&pick_generic, path, line("ov.pick(o)"));
    assert_kept(&pick_string, path, line("ov.pick(o)"));
}

#[test]
fn a_java_construction_binds_the_constructor_its_argument_types_select() {
    let repo = scanned_repo(&[
        (
            "src/p/Box.java",
            "package p;\n\nclass Box {\n    Box(int a) {}\n    Box(String s) {}\n}\n",
        ),
        (
            "src/p/Use.java",
            "package p;\n\nclass Use {\n    Box one() { return new Box(1); }\n    Box two() { return new Box(\"s\"); }\n    Box three(Object o) { return new Box(o); }\n}\n",
        ),
    ]);
    let path = "src/p/Use.java";
    let by_int = report_of(&repo, "Box", "src/p/Box.java", 0);
    let by_string = report_of(&repo, "Box", "src/p/Box.java", 1);
    assert_kept(&by_int, path, 4);
    assert_eq!(
        site(&by_string, path, 4).0,
        SiteLabel::Excluded,
        "{}",
        dump(&by_string)
    );
    assert_eq!(
        site(&by_int, path, 5).0,
        SiteLabel::Excluded,
        "{}",
        dump(&by_int)
    );
    assert_kept(&by_string, path, 5);
    assert_kept(&by_int, path, 6);
    assert_kept(&by_string, path, 6);

    assert_eq!(
        impacted(&repo, &symbol_id(&repo, "Box", "src/p/Box.java", 0)),
        ["one", "three"]
    );
    assert_eq!(
        impacted(&repo, &symbol_id(&repo, "Box", "src/p/Box.java", 1)),
        ["three", "two"]
    );
}

#[test]
fn a_kotlin_call_binds_by_types_and_named_arguments_and_keeps_overloads_that_take_a_null() {
    let repo = scanned_repo(&[
        (
            "src/Ov.kt",
            "class Ov {\n    fun run(a: Int) {}\n    fun run(a: String) {}\n    fun put(a: Int, b: String = \"\") {}\n    fun put(c: String) {}\n    fun show(x: Int) {}\n    fun show(x: Any?) {}\n    fun box(a: Any) {}\n    fun box(a: String) {}\n}\n",
        ),
        (
            "src/Use.kt",
            "fun go(ov: Ov, n: Int?, wide: Long, o: Any) {\n    ov.run(1)\n    ov.run(\"x\")\n    ov.run(wide)\n    ov.run(o)\n    ov.put(c = \"x\")\n    ov.put(b = \"y\", a = 2)\n    ov.show(n)\n    ov.box(1)\n}\n",
        ),
    ]);
    let path = "src/Use.kt";
    let run_int = report_of(&repo, "run", "src/Ov.kt", 0);
    let run_string = report_of(&repo, "run", "src/Ov.kt", 1);
    assert_excluded(
        &run_string,
        path,
        2,
        "arguments `(Int)` bind only `run(Int)`",
    );
    assert_excluded(
        &run_int,
        path,
        3,
        "arguments `(String)` bind only `run(String)`",
    );
    for line in [4, 5] {
        assert_kept(&run_int, path, line);
        assert_kept(&run_string, path, line);
    }

    let put_ab = report_of(&repo, "put", "src/Ov.kt", 0);
    let put_c = report_of(&repo, "put", "src/Ov.kt", 1);
    assert_excluded(
        &put_ab,
        path,
        6,
        "arguments `(c = String)` bind only `put(String)`",
    );
    assert_kept(&put_c, path, 6);
    assert_excluded(
        &put_c,
        path,
        7,
        "arguments `(b = String, a = Int)` bind only `put(Int, String)`",
    );
    assert_kept(&put_ab, path, 7);

    let show_int = report_of(&repo, "show", "src/Ov.kt", 0);
    let show_any = report_of(&repo, "show", "src/Ov.kt", 1);
    assert_kept(&show_int, path, 8);
    assert_kept(&show_any, path, 8);

    let box_any = report_of(&repo, "box", "src/Ov.kt", 0);
    let box_string = report_of(&repo, "box", "src/Ov.kt", 1);
    assert_kept(&box_any, path, 9);
    assert_excluded(
        &box_string,
        path,
        9,
        "arguments `(Int)` bind only `box(Any)`",
    );
}

#[test]
fn a_scala_call_binds_by_exact_types_and_names_and_keeps_conversions_undecided() {
    let repo = scanned_repo(&[
        (
            "src/Ov.scala",
            "class Ov {\n  def run(a: Int): Unit = ()\n  def run(a: String): Unit = ()\n  def opt(a: String, b: Int = 1): Unit = ()\n  def opt(c: Int): Unit = ()\n}\n",
        ),
        (
            "src/Use.scala",
            "object Use {\n  def go(ov: Ov, wide: Long): Unit = {\n    ov.run(1)\n    ov.run(\"x\")\n    ov.run(wide)\n    ov.opt(c = 2)\n    ov.opt(\"s\")\n  }\n}\n",
        ),
    ]);
    let path = "src/Use.scala";
    let run_int = report_of(&repo, "run", "src/Ov.scala", 0);
    let run_string = report_of(&repo, "run", "src/Ov.scala", 1);
    assert_excluded(
        &run_string,
        path,
        3,
        "arguments `(Int)` bind only `run(Int)`",
    );
    assert_excluded(
        &run_int,
        path,
        4,
        "arguments `(String)` bind only `run(String)`",
    );
    assert_kept(&run_int, path, 5);
    assert_kept(&run_string, path, 5);

    let opt_ab = report_of(&repo, "opt", "src/Ov.scala", 0);
    let opt_c = report_of(&repo, "opt", "src/Ov.scala", 1);
    assert_excluded(
        &opt_ab,
        path,
        6,
        "arguments `(c = Int)` bind only `opt(Int)`",
    );
    assert_excluded(
        &opt_c,
        path,
        7,
        "arguments `(String)` bind only `opt(String, Int)`",
    );
}

#[test]
fn a_csharp_call_binds_only_by_exact_types() {
    let repo = scanned_repo(&[
        (
            "Ov.cs",
            "public class Ov {\n    public void Run(int a) {}\n    public void Run(string a) {}\n}\n",
        ),
        (
            "Use.cs",
            "public class Use {\n    public void Go(Ov ov, long wide, object o) {\n        ov.Run(1);\n        ov.Run(\"x\");\n        ov.Run(wide);\n        ov.Run(o);\n    }\n}\n",
        ),
    ]);
    let run_int = report_of(&repo, "Run", "Ov.cs", 0);
    let run_string = report_of(&repo, "Run", "Ov.cs", 1);
    assert_excluded(
        &run_string,
        "Use.cs",
        3,
        "arguments `(int)` bind only `Run(int)`",
    );
    assert_excluded(
        &run_int,
        "Use.cs",
        4,
        "arguments `(string)` bind only `Run(string)`",
    );
    for line in [5, 6] {
        assert_kept(&run_int, "Use.cs", line);
        assert_kept(&run_string, "Use.cs", line);
    }
}

const CSHARP_NULLABLE_OV: &str = "public struct Pt {}\npublic class Box {}\npublic class Ov {\n    public void Nul(int a) {}\n    public void Nul(int? a) {}\n    public void Num(int a) {}\n    public void Num(object a) {}\n    public void Num(string a) {}\n    public void P(Pt p) {}\n    public void P(object o) {}\n    public void B(Box b) {}\n    public void B(object o) {}\n}\n";
const CSHARP_NULLABLE_USE: &str = "public class Use {\n    private int? field;\n    public void Go(Ov ov, int? n, Pt? maybe, Box? box) {\n        int? local = null;\n        ov.Nul(local);\n        ov.Nul(field);\n        ov.Nul(n);\n        ov.Nul((int?)3);\n        ov.Num(n);\n        ov.Num(\"s\");\n        ov.P(maybe);\n        ov.B(box);\n    }\n}\n";

#[test]
fn a_csharp_nullable_value_that_julie_writes_without_its_marker_keeps_the_overloads_that_take_it() {
    let repo = scanned_repo(&[
        ("Ov.cs", CSHARP_NULLABLE_OV),
        ("Use.cs", CSHARP_NULLABLE_USE),
    ]);
    let path = "Use.cs";
    let line = |needle| line_of(&repo, path, needle);
    let nul_value = report_of(&repo, "Nul", "Ov.cs", 0);
    let nul_nullable = report_of(&repo, "Nul", "Ov.cs", 1);
    for call in ["ov.Nul(local)", "ov.Nul(field)", "ov.Nul(n)"] {
        assert_kept(&nul_value, path, line(call));
        assert_kept(&nul_nullable, path, line(call));
    }
    assert_kept(&nul_nullable, path, line("ov.Nul((int?)3)"));
    assert_excluded(
        &nul_value,
        path,
        line("ov.Nul((int?)3)"),
        "arguments `(int?)` bind only `Nul(int?)`",
    );

    let num_object = report_of(&repo, "Num", "Ov.cs", 1);
    assert_kept(&num_object, path, line("ov.Num(n)"));
    assert_excluded(
        &num_object,
        path,
        line("ov.Num(\"s\")"),
        "arguments `(string)` bind only `Num(string)`",
    );

    let p_object = report_of(&repo, "P", "Ov.cs", 1);
    assert_kept(&p_object, path, line("ov.P(maybe)"));

    let b_object = report_of(&repo, "B", "Ov.cs", 1);
    assert_excluded(
        &b_object,
        path,
        line("ov.B(box)"),
        "arguments `(Box)` bind only `B(Box)`",
    );
}

const CSHARP_API: &str = "#nullable enable\nusing System;\npublic class Settings {}\npublic class Base {}\npublic class Sub : Base {}\npublic class Api {\n    public static string Parse(string json)\n    {\n        return Parse(json, null);\n    }\n    public static string Parse(string json, Settings? settings) { return json; }\n    public static string Ser(object? value)\n    {\n        return Ser(value, null, (Settings?)null);\n    }\n    public static string Ser(object? value, Settings? settings)\n    {\n        return Ser(value, null, settings);\n    }\n    public static string Ser(object? value, Type? type, Settings? settings) { return \"\"; }\n    public void Run(object? a) {}\n    public void Run(string a) {}\n    public void Take(Base b) {}\n    public void Take(int a) {}\n    public void Opt(int a, int b = 0) {}\n    public void Opt(string a) {}\n    public void Par(params int[] xs) {}\n    public void Par(string a) {}\n    public void Dyn(dynamic d) {}\n    public void Dyn(string s) {}\n    public void Def(int a, int b = 0) {}\n    public void Def(in int a) {}\n    public void Gen(int a) {}\n    public void Gen<T>(int a) {}\n}\n";
const CSHARP_API_USE: &str = "#nullable enable\npublic class Use {\n    public void Go(Api api, object? o, string? s, Sub sub, dynamic d, int x) {\n        api.Run(o);\n        api.Run(s);\n        api.Run(null);\n        api.Take(sub);\n        api.Opt(1);\n        api.Par(1, 2);\n        api.Par(1);\n        api.Dyn(d);\n        api.Def(x);\n        api.Gen(1);\n    }\n}\n";

#[test]
fn a_csharp_call_keeps_overloads_that_annotations_nulls_subtypes_params_dynamic_or_twins_leave_open()
 {
    let repo = scanned_repo(&[("Api.cs", CSHARP_API), ("Use.cs", CSHARP_API_USE)]);
    let path = "Use.cs";
    let line = |needle| line_of(&repo, path, needle);

    let run_object = report_of(&repo, "Run", "Api.cs", 0);
    let run_string = report_of(&repo, "Run", "Api.cs", 1);
    assert_kept(&run_object, path, line("api.Run(o)"));
    assert_excluded(
        &run_string,
        path,
        line("api.Run(o)"),
        "arguments `(object)` bind only `Run(object?)`",
    );
    assert_kept(&run_string, path, line("api.Run(s)"));
    assert_excluded(
        &run_object,
        path,
        line("api.Run(s)"),
        "arguments `(string)` bind only `Run(string)`",
    );
    assert_kept(&run_object, path, line("api.Run(null)"));
    assert_kept(&run_string, path, line("api.Run(null)"));

    for nth in [0, 1] {
        assert_kept(
            &report_of(&repo, "Take", "Api.cs", nth),
            path,
            line("api.Take(sub)"),
        );
        assert_kept(
            &report_of(&repo, "Par", "Api.cs", nth),
            path,
            line("api.Par(1)"),
        );
        assert_kept(
            &report_of(&repo, "Dyn", "Api.cs", nth),
            path,
            line("api.Dyn(d)"),
        );
        assert_kept(
            &report_of(&repo, "Def", "Api.cs", nth),
            path,
            line("api.Def(x)"),
        );
        assert_kept(
            &report_of(&repo, "Gen", "Api.cs", nth),
            path,
            line("api.Gen(1)"),
        );
    }
    assert_excluded(
        &report_of(&repo, "Opt", "Api.cs", 1),
        path,
        line("api.Opt(1)"),
        "arguments `(int)` bind only `Opt(int, int)`",
    );
    assert_eq!(
        site(
            &report_of(&repo, "Par", "Api.cs", 1),
            path,
            line("api.Par(1, 2)")
        )
        .0,
        SiteLabel::Excluded
    );
}

#[test]
fn a_csharp_call_whose_argument_count_only_another_overload_takes_stays_excluded() {
    let repo = scanned_repo(&[("Api.cs", CSHARP_API)]);
    let path = "Api.cs";
    let line = |needle| line_of(&repo, path, needle);
    let parse_call = line("return Parse(json, null)");
    assert_excluded(
        &report_of(&repo, "Parse", path, 0),
        path,
        parse_call,
        "arguments `(string, ?)` bind only `Parse(string, Settings?)`",
    );
    assert_kept(&report_of(&repo, "Parse", path, 1), path, parse_call);

    let three = line("return Ser(value, null, (Settings?)null)");
    let forwarded = line("return Ser(value, null, settings)");
    for target in [0, 1] {
        let report = report_of(&repo, "Ser", path, target);
        for call in [three, forwarded] {
            assert_eq!(
                site(&report, path, call).0,
                SiteLabel::Excluded,
                "{}",
                dump(&report)
            );
        }
    }
    let by_type = report_of(&repo, "Ser", path, 2);
    assert_kept(&by_type, path, three);
    assert_kept(&by_type, path, forwarded);
}

#[test]
fn an_overload_lists_only_the_callers_whose_arguments_can_bind_it() {
    let cases = [
        (
            "src/Ov.java",
            "class Ov {\n  static int run(int a) { return a; }\n  static int run(String a) { return 1; }\n  static int first() { return run(1); }\n  static int second() { return run(\"x\"); }\n  static int third() { return run(null); }\n}\n",
        ),
        (
            "src/Ov.cs",
            "class Ov {\n  static int Run(int a) { return a; }\n  static int Run(string a) { return 1; }\n  static int First() { return Run(1); }\n  static int Second() { return Run(\"x\"); }\n  static int Third(object o) { return Run(o); }\n}\n",
        ),
        (
            "src/ov.kt",
            "fun run(a: Int): Int = a\nfun run(a: String): Int = 1\nfun first(): Int = run(1)\nfun second(): Int = run(\"x\")\nfun third(o: Any): Int = run(o)\n",
        ),
        (
            "src/ov.scala",
            "object Ov {\n  def run(a: Int): Int = a\n  def run(a: String): Int = 1\n  def first(): Int = run(1)\n  def second(): Int = run(\"x\")\n  def third(o: Long): Int = run(o)\n}\n",
        ),
        (
            "src/ov.cpp",
            "int make();\nint run(int a) { return a; }\nint run(const char* a) { return 1; }\nint first() { return run(1); }\nint second() { return run(\"x\"); }\nint third() { return run(make()); }\n",
        ),
        (
            "src/ov.swift",
            "func make() -> Int { return 1 }\nfunc run(_ a: Int) -> Int { return a }\nfunc run(_ a: String) -> Int { return 1 }\nfunc first() -> Int { return run(1) }\nfunc second() -> Int { return run(\"x\") }\nfunc third() -> Int { return run(make()) }\n",
        ),
        (
            "src/Ov.vb",
            "Public Module Ov\n    Function Make() As Integer\n        Return 1\n    End Function\n    Function Run(a As Integer) As Integer\n        Return a\n    End Function\n    Function Run(a As String) As Integer\n        Return 1\n    End Function\n    Function First() As Integer\n        Return Run(1)\n    End Function\n    Function Second() As Integer\n        Return Run(\"x\")\n    End Function\n    Function Third() As Integer\n        Return Run(Make())\n    End Function\nEnd Module\n",
        ),
    ];
    for (path, source) in cases {
        let repo = scanned_repo(&[(path, source)]);
        let name = if path.ends_with(".cs") || path.ends_with(".vb") {
            "Run"
        } else {
            "run"
        };
        assert_eq!(
            impacted(&repo, &symbol_id(&repo, name, path, 0)),
            ["first", "third"],
            "{path}"
        );
        assert_eq!(
            impacted(&repo, &symbol_id(&repo, name, path, 1)),
            ["second", "third"],
            "{path}"
        );
    }
}

#[test]
fn a_java_call_bound_to_another_overload_is_no_site_of_a_changed_overload() {
    let repo = scanned_repo(&[
        (
            "src/p/Picker.java",
            "package p;\n\nclass Picker {\n    void pick(int v) {}\n    void pick(String v) {}\n}\n",
        ),
        (
            "src/p/Use.java",
            "package p;\n\nclass Use {\n    void byInt(Picker p) { p.pick(1); }\n    void byString(Picker p) { p.pick(\"x\"); }\n    void unknown(Picker p, Object o) { p.pick(o); }\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/p/Picker.java",
        "package p;\n\nclass Picker {\n    void pick(int v, int w) {}\n    void pick(String v) {}\n}\n",
    );
    let text = check_text(&repo);
    assert!(text.contains("`byInt`"), "{text}");
    assert!(!text.contains("`byString`"), "{text}");
    assert!(text.contains("`unknown`"), "{text}");
}

#[test]
fn a_kotlin_call_bound_to_another_overload_by_name_is_no_site_of_a_changed_overload() {
    let repo = scanned_repo(&[
        (
            "src/Picker.kt",
            "class Picker {\n    fun pick(a: Int) {}\n    fun pick(c: String) {}\n}\n",
        ),
        (
            "src/Use.kt",
            "fun byInt(p: Picker) { p.pick(1) }\nfun byName(p: Picker) { p.pick(c = \"x\") }\nfun byString(p: Picker) { p.pick(\"x\") }\n",
        ),
    ]);
    write(
        &repo.root,
        "src/Picker.kt",
        "class Picker {\n    fun pick(a: Int, b: Int) {}\n    fun pick(c: String) {}\n}\n",
    );
    let text = check_text(&repo);
    assert!(text.contains("`byInt`"), "{text}");
    assert!(!text.contains("`byName`"), "{text}");
    assert!(!text.contains("`byString`"), "{text}");
}

#[test]
fn a_scala_call_bound_to_another_overload_is_no_site_of_a_changed_overload() {
    let repo = scanned_repo(&[
        (
            "src/Picker.scala",
            "class Picker {\n  def pick(a: Int): Unit = ()\n  def pick(a: String): Unit = ()\n}\n",
        ),
        (
            "src/Use.scala",
            "object Use {\n  def byInt(p: Picker): Unit = p.pick(1)\n  def byString(p: Picker): Unit = p.pick(\"x\")\n}\n",
        ),
    ]);
    write(
        &repo.root,
        "src/Picker.scala",
        "class Picker {\n  def pick(a: Int, b: Int): Unit = ()\n  def pick(a: String): Unit = ()\n}\n",
    );
    let text = check_text(&repo);
    assert!(text.contains("`byInt`"), "{text}");
    assert!(!text.contains("`byString`"), "{text}");
}

#[test]
fn an_override_keeps_a_call_through_its_base_that_uses_the_base_default() {
    let repo = scanned_repo(&[
        (
            "src/Ov.kt",
            "open class Base {\n    open fun run(a: Int, b: Int = 0) {}\n}\nclass Derived : Base() {\n    override fun run(a: Int, b: Int) {}\n    fun run(s: String) {}\n}\n",
        ),
        (
            "src/Use.kt",
            "fun go(base: Base, derived: Derived) {\n    base.run(1)\n    derived.run(1)\n    derived.run(\"x\")\n}\n",
        ),
        (
            "Ov.cs",
            "public class Base {\n    public virtual void Run(int a, int b = 0) {}\n}\npublic class Derived : Base {\n    public override void Run(int a, int b) {}\n    public void Run(string s) {}\n}\n",
        ),
        (
            "Use.cs",
            "public class Use {\n    public void Go(Base b, Derived d) {\n        b.Run(1);\n        d.Run(1);\n        d.Run(\"x\");\n    }\n}\n",
        ),
    ]);
    let kotlin = report_of(&repo, "run", "src/Ov.kt", 1);
    assert_not_bound_elsewhere(&kotlin, "src/Use.kt", 2);
    assert_kept(&kotlin, "src/Use.kt", 3);
    assert_excluded(
        &kotlin,
        "src/Use.kt",
        4,
        "arguments `(String)` bind only `run(String)`",
    );
    let csharp = report_of(&repo, "Run", "Ov.cs", 1);
    assert_not_bound_elsewhere(&csharp, "Use.cs", 3);
    assert_kept(&csharp, "Use.cs", 4);
    assert_excluded(
        &csharp,
        "Use.cs",
        5,
        "arguments `(string)` bind only `Run(string)`",
    );
}

#[test]
fn kotlin_and_scala_secondary_constructors_leave_the_primary_calls_to_the_primary_constructor() {
    let repo = scanned_repo(&[
        (
            "src/Run.kt",
            "class Run(val v: Int) {\n    constructor(s: String) : this(1)\n}\n",
        ),
        (
            "src/UseRun.kt",
            "fun byInt(): Run = Run(1)\nfun byString(): Run = Run(\"s\")\n",
        ),
        (
            "src/Cell.scala",
            "class Cell(v: Int) {\n  def this(s: String) = this(1)\n}\n",
        ),
        (
            "src/UseCell.scala",
            "object UseCell {\n  def byInt(): Cell = new Cell(1)\n  def byString(): Cell = new Cell(\"s\")\n}\n",
        ),
    ]);
    let kotlin = report_of(&repo, "Run", "src/Run.kt", 0);
    assert_eq!(
        site(&kotlin, "src/UseRun.kt", 1).0,
        SiteLabel::Excluded,
        "{}",
        dump(&kotlin)
    );
    assert_kept(&kotlin, "src/UseRun.kt", 2);
    let scala = report_of(&repo, "Cell", "src/Cell.scala", 0);
    assert_eq!(
        site(&scala, "src/UseCell.scala", 2).0,
        SiteLabel::Excluded,
        "{}",
        dump(&scala)
    );
    assert_kept(&scala, "src/UseCell.scala", 3);
}

#[test]
fn java_and_scala_new_expressions_stay_constructions_and_constructor_sites() {
    let repo = scanned_repo(&[
        (
            "src/p/Box.java",
            "package p;\n\nclass Box {\n    Box(int a) {}\n}\n",
        ),
        (
            "src/p/Use.java",
            "package p;\n\nclass Use {\n    Box make() {\n        Box box = new Box(1);\n        return box;\n    }\n}\n",
        ),
        ("src/Cell.scala", "class Cell(v: Int)\n"),
        (
            "src/UseCell.scala",
            "object UseCell {\n  def make(): Cell = {\n    val cell = new Cell(1)\n    cell\n  }\n}\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    for (name, path, use_path, line) in [
        ("Box", "src/p/Box.java", "src/p/Use.java", 5),
        ("Cell", "src/Cell.scala", "src/UseCell.scala", 3),
    ] {
        let class_id: String = conn
            .query_row(
                "SELECT symbol_id FROM symbols WHERE name = ?1 AND path = ?2 AND kind = 'class'",
                [name, path],
                |row| row.get(0),
            )
            .unwrap();
        let class = get_symbol_by_id(&conn, &class_id).unwrap().unwrap();
        let report = reference_report(&conn, &repo.root, &class, false).unwrap();
        assert!(
            report
                .constructions
                .iter()
                .any(|row| row.path == use_path && row.line == line),
            "{name}: {:?}\n{}",
            report.constructions,
            dump(&report)
        );
        assert_kept(&report, use_path, line);
    }
    let constructor = report_of(&repo, "Box", "src/p/Box.java", 0);
    assert_kept(&constructor, "src/p/Use.java", 5);
}

#[test]
fn a_count_that_one_overload_takes_binds_it_in_cpp() {
    let cases = [(
        "src/ov.cpp",
        "int run(int a) { return a; }\nint run(int a, int b) { return b; }\nint first() { return run(1); }\nint second() { return run(1, 2); }\n",
    )];
    for (path, source) in cases {
        let repo = scanned_repo(&[(path, source)]);
        let one = report_of(&repo, "run", path, 0);
        let two = report_of(&repo, "run", path, 1);
        assert_kept(&one, path, 3);
        assert_eq!(site(&one, path, 4).0, SiteLabel::Excluded, "{}", dump(&one));
        assert_eq!(site(&two, path, 3).0, SiteLabel::Excluded, "{}", dump(&two));
        assert_kept(&two, path, 4);
        assert_eq!(
            impacted(&repo, &symbol_id(&repo, "run", path, 0)),
            ["first"],
            "{path}"
        );
        assert_eq!(
            impacted(&repo, &symbol_id(&repo, "run", path, 1)),
            ["second"],
            "{path}"
        );
    }
}

const CPP_RUN: &str = "struct Point { int x; };\nint make();\nint run(int a) { return a; }\nint run(const char* a) { return 1; }\nint run(const Point& p) { return 2; }\nint run(long a, int b = 0) { return 3; }\nint first() { return run(1); }\nint second() { return run(\"x\"); }\nint third() { Point p; return run(p); }\nint fourth() { return run(make()); }\nint fifth() { short s = 1; return run(s); }\n";

#[test]
fn a_cpp_call_binds_by_exact_types_and_keeps_promotions_and_unknown_types() {
    let path = "src/ov.cpp";
    let repo = scanned_repo(&[(path, CPP_RUN)]);
    let by_int = report_of(&repo, "run", path, 0);
    assert_kept(&by_int, path, 7);
    assert_excluded(
        &by_int,
        path,
        8,
        "arguments `(const char*)` bind only `run(const char*)`",
    );
    assert_excluded(
        &by_int,
        path,
        9,
        "arguments `(Point)` bind only `run(const Point&)`",
    );
    assert_kept(&by_int, path, 10);
    assert_kept(&by_int, path, 11);

    let by_point = report_of(&repo, "run", path, 2);
    assert_kept(&by_point, path, 9);
    assert_excluded(&by_point, path, 7, "arguments `(int)` bind only `run(int)`");

    let by_long = report_of(&repo, "run", path, 3);
    assert_excluded(&by_long, path, 7, "arguments `(int)` bind only `run(int)`");
    assert_kept(&by_long, path, 10);
    assert_kept(&by_long, path, 11);
}

#[test]
fn a_cpp_construction_binds_the_constructor_its_argument_types_select() {
    let path = "src/box.cpp";
    let repo = scanned_repo(&[(
        path,
        "class Box {\npublic:\n    Box(int a) {}\n    Box(const char* s) {}\n};\nint count() {\n    Box b(1);\n    return 0;\n}\nint braced() {\n    Box d{2};\n    return 0;\n}\nBox* text() {\n    return new Box(\"s\");\n}\n",
    )]);
    let by_int = report_of(&repo, "Box", path, 0);
    assert_kept(&by_int, path, 7);
    assert_kept(&by_int, path, 11);
    assert_excluded(
        &by_int,
        path,
        15,
        "arguments `(const char*)` bind only `Box(const char*)`",
    );
    let by_text = report_of(&repo, "Box", path, 1);
    assert_excluded(&by_text, path, 7, "arguments `(int)` bind only `Box(int)`");
    assert_kept(&by_text, path, 15);

    let conn = open_read_only(&repo.db).unwrap();
    let class_id: String = conn
        .query_row(
            "SELECT symbol_id FROM symbols WHERE name = 'Box' AND path = ?1 AND kind = 'class'",
            [path],
            |row| row.get(0),
        )
        .unwrap();
    let class = get_symbol_by_id(&conn, &class_id).unwrap().unwrap();
    let report = reference_report(&conn, &repo.root, &class, false).unwrap();
    let mut lines: Vec<usize> = report.constructions.iter().map(|row| row.line).collect();
    lines.sort_unstable();
    assert_eq!(lines, [7, 11, 15], "{:?}", report.constructions);

    assert_eq!(
        impacted(&repo, &symbol_id(&repo, "Box", path, 0)),
        ["braced", "count"]
    );
    assert_eq!(impacted(&repo, &symbol_id(&repo, "Box", path, 1)), ["text"]);
}

#[test]
fn a_swift_call_binds_by_labels_first_then_types_and_keeps_a_trailing_closure() {
    let path = "src/ov.swift";
    let repo = scanned_repo(&[(
        path,
        "func make() -> Int { return 1 }\nfunc run(a: Int) -> Int { return a }\nfunc run(b: Int) -> Int { return b }\nfunc run(_ a: Int) -> Int { return a }\nfunc run(_ a: String) -> Int { return 1 }\nfunc run(_ a: Int, with f: () -> Int) -> Int { return f() }\nfunc first() -> Int { return run(a: 1) }\nfunc second() -> Int { return run(b: 2) }\nfunc third() -> Int { return run(1) }\nfunc fourth() -> Int { return run(\"x\") }\nfunc fifth() -> Int { return run(make()) }\nfunc sixth() -> Int { return run(1) { 2 } }\n",
    )]);
    let labeled = report_of(&repo, "run", path, 0);
    assert_kept(&labeled, path, 7);
    assert_excluded(
        &labeled,
        path,
        8,
        "arguments `(b: Int)` bind only `run(b: Int)`",
    );
    assert_excluded(
        &labeled,
        path,
        9,
        "arguments `(Int)` bind only `run(_: Int)`",
    );
    assert_excluded(
        &labeled,
        path,
        11,
        "arguments `(?)` bind only `run(_: Int)` or `run(_: String)`",
    );

    let unlabeled = report_of(&repo, "run", path, 2);
    assert_excluded(
        &unlabeled,
        path,
        7,
        "arguments `(a: Int)` bind only `run(a: Int)`",
    );
    assert_kept(&unlabeled, path, 9);
    assert_excluded(
        &unlabeled,
        path,
        10,
        "arguments `(String)` bind only `run(_: String)`",
    );
    assert_kept(&unlabeled, path, 11);

    let closure = report_of(&repo, "run", path, 4);
    assert_kept(&closure, path, 12);
    assert_excluded(
        &closure,
        path,
        9,
        "arguments `(Int)` bind only `run(_: Int)`",
    );
}

#[test]
fn a_vbnet_call_binds_by_types_and_keeps_named_arguments_and_unknown_types() {
    let path = "src/Ov.vb";
    let repo = scanned_repo(&[(
        path,
        "Public Class Ov\n    Public Sub New(a As Integer)\n    End Sub\n    Public Sub New(s As String)\n    End Sub\n    Public Function Run(a As Integer) As Integer\n        Return a\n    End Function\n    Public Function Run(a As String) As Integer\n        Return 1\n    End Function\n    Public Function Run(a As Long, Optional b As Long = 0) As Integer\n        Return 2\n    End Function\n    Public Function Make() As Integer\n        Return 1\n    End Function\n    Public Function First() As Integer\n        Return Run(1)\n    End Function\n    Public Function Second() As Integer\n        Return Run(\"x\")\n    End Function\n    Public Function Third() As Integer\n        Return Run(Make())\n    End Function\n    Public Function Fourth() As Integer\n        Return Run(a:=1)\n    End Function\n    Public Shared Function Fifth() As Ov\n        Return New Ov(1)\n    End Function\nEnd Class\n",
    )]);
    let by_integer = report_of(&repo, "Run", path, 0);
    assert_kept(&by_integer, path, 19);
    assert_excluded(
        &by_integer,
        path,
        22,
        "arguments `(String)` bind only `Run(String)`",
    );
    assert_kept(&by_integer, path, 25);
    assert_kept(&by_integer, path, 28);

    let by_long = report_of(&repo, "Run", path, 2);
    assert_excluded(
        &by_long,
        path,
        19,
        "arguments `(Integer)` bind only `Run(Integer)`",
    );
    assert_kept(&by_long, path, 25);
    assert_kept(&by_long, path, 28);

    assert_kept(&report_of(&repo, "New", path, 0), path, 31);
    assert_excluded(
        &report_of(&repo, "New", path, 1),
        path,
        31,
        "arguments `(Integer)` bind only `Ov(Integer)`",
    );
}

const VBNET_LATE: &str = "Public Class Late\n    Public Function Run(a As Object) As Integer\n        Return 1\n    End Function\n    Public Function Run(a As String) As Integer\n        Return 2\n    End Function\n    Public Function Take(a As Integer) As Integer\n        Return 1\n    End Function\n    Public Function Take(a As String) As Integer\n        Return 2\n    End Function\n    Public Function Go(o As Object, s As String) As Integer\n        Run(o)\n        Run(s)\n        Run(Nothing)\n        Take(o)\n        Return 0\n    End Function\nEnd Class\n";

#[test]
fn a_vbnet_object_argument_binds_the_object_overload_and_binds_late_only_without_one() {
    let path = "src/Late.vb";
    let repo = scanned_repo(&[(path, VBNET_LATE)]);
    let line = |needle| line_of(&repo, path, needle);
    let by_object = report_of(&repo, "Run", path, 0);
    let by_string = report_of(&repo, "Run", path, 1);
    assert_kept(&by_object, path, line("Run(o)"));
    assert_excluded(
        &by_string,
        path,
        line("Run(o)"),
        "arguments `(Object)` bind only `Run(Object)`",
    );
    assert_kept(&by_string, path, line("Run(s)"));
    assert_excluded(
        &by_object,
        path,
        line("Run(s)"),
        "arguments `(String)` bind only `Run(String)`",
    );
    assert_kept(&by_object, path, line("Run(Nothing)"));
    assert_kept(&by_string, path, line("Run(Nothing)"));
    for nth in [0, 1] {
        assert_kept(&report_of(&repo, "Take", path, nth), path, line("Take(o)"));
    }
}

const CSHARP_NEVER_NULL_OV: &str = "using System;\nnamespace App {\n    public enum Mode { A, B }\n    public static class Lib {\n        public static void Lit(int x) { }\n        public static void Lit(long x) { }\n        public static void Lit(int? x) { }\n        public static void Lit(Uri u) { }\n        public static void Lit(IComparable c) { }\n        public static void Ext(Uri u) { }\n        public static void Ext(Uri? u, int extra = 0) { }\n        public static void Md(Mode m) { }\n        public static void Md(Mode m, int extra = 0) { }\n        public static void Ob(int x) { }\n        public static void Ob(object x) { }\n    }\n}\n";
const CSHARP_NEVER_NULL_USE: &str = "#nullable enable\nusing System;\nnamespace App {\n    public class Use {\n        public void Go(Uri? maybe, Mode m, int? x) {\n            Lib.Lit(x);\n            Lib.Ext(maybe);\n            Lib.Md(m);\n            Lib.Ob(x);\n        }\n    }\n}\n";

#[test]
fn a_csharp_overload_that_cannot_take_the_hidden_nullable_value_stays_excluded() {
    let repo = scanned_repo(&[
        ("Lib.cs", CSHARP_NEVER_NULL_OV),
        ("Use.cs", CSHARP_NEVER_NULL_USE),
    ]);
    let path = "Use.cs";
    let line = |needle| line_of(&repo, path, needle);
    for nth in [0, 2, 4] {
        assert_kept(
            &report_of(&repo, "Lit", "Lib.cs", nth),
            path,
            line("Lib.Lit(x)"),
        );
    }
    for nth in [1, 3] {
        assert_excluded(
            &report_of(&repo, "Lit", "Lib.cs", nth),
            path,
            line("Lib.Lit(x)"),
            "arguments `(int)` bind only `Lit(int)`, `Lit(int?)`, or 1 more",
        );
    }

    assert_kept(
        &report_of(&repo, "Ext", "Lib.cs", 0),
        path,
        line("Lib.Ext(maybe)"),
    );
    assert_excluded(
        &report_of(&repo, "Ext", "Lib.cs", 1),
        path,
        line("Lib.Ext(maybe)"),
        "arguments `(Uri)` bind only `Ext(Uri)`",
    );

    assert_kept(
        &report_of(&repo, "Md", "Lib.cs", 0),
        path,
        line("Lib.Md(m)"),
    );
    assert_excluded(
        &report_of(&repo, "Md", "Lib.cs", 1),
        path,
        line("Lib.Md(m)"),
        "arguments `(Mode)` bind only `Md(Mode)`",
    );

    for nth in [0, 1] {
        assert_kept(
            &report_of(&repo, "Ob", "Lib.cs", nth),
            path,
            line("Lib.Ob(x)"),
        );
    }
}

#[test]
fn a_qualified_and_a_simple_name_of_one_type_never_bind_the_overload_with_a_default() {
    let csharp = scanned_repo(&[(
        "Lib.cs",
        "namespace App {\n    public class Box { }\n    public static class Lib {\n        public static void R(App.Box b) { }\n        public static void R(Box b, int extra = 0) { }\n    }\n    public class Use {\n        public void Go(Box b) {\n            Lib.R(b);\n        }\n    }\n}\n",
    )]);
    let call = line_of(&csharp, "Lib.cs", "Lib.R(b)");
    assert_kept(&report_of(&csharp, "R", "Lib.cs", 0), "Lib.cs", call);
    assert_excluded(
        &report_of(&csharp, "R", "Lib.cs", 1),
        "Lib.cs",
        call,
        "arguments `(Box)` bind only `R(App.Box)`",
    );

    let cases = [
        (
            "src/ov.cpp",
            "put",
            "namespace ns {\nstruct Box {};\nint put(ns::Box b) { return 1; }\nint put(Box b, int extra = 0) { return 2; }\nint use() {\n    Box box;\n    return put(box);\n}\n}\n",
            "return put(box)",
        ),
        (
            "src/Ov.vb",
            "Put",
            "Namespace App\n    Public Class Box\n    End Class\n    Public Class Ov\n        Public Function Put(b As App.Box) As Integer\n            Return 1\n        End Function\n        Public Function Put(b As Box, Optional extra As Integer = 0) As Integer\n            Return 2\n        End Function\n        Public Function Go(box As Box) As Integer\n            Return Put(box)\n        End Function\n    End Class\nEnd Namespace\n",
            "Return Put(box)",
        ),
        (
            "src/p/Ov.kt",
            "put",
            "package p\n\nclass Box\n\nclass Ov {\n    fun put(b: p.Box): Int = 1\n    fun put(b: Box, extra: Int = 0): Int = 2\n    fun go(box: Box): Int {\n        return put(box)\n    }\n}\n",
            "return put(box)",
        ),
    ];
    for (path, name, source, needle) in cases {
        let repo = scanned_repo(&[(path, source)]);
        let call = line_of(&repo, path, needle);
        assert_kept(&report_of(&repo, name, path, 0), path, call);
        assert_not_bound_elsewhere(&report_of(&repo, name, path, 0), path, call);
    }
}

#[test]
fn a_simple_name_of_two_project_types_binds_only_the_type_its_imports_name() {
    let cases = [
        (
            vec![
                ("src/a/Node.java", "package a;\n\npublic class Node {}\n"),
                ("src/b/Node.java", "package b;\n\npublic class Node {}\n"),
                (
                    "src/c/Lib.java",
                    "package c;\n\nimport a.Node;\n\npublic class Lib {\n    public static void take(Node n) {}\n    public static void take(Object o) {}\n}\n",
                ),
                (
                    "src/c/UseB.java",
                    "package c;\n\nimport b.Node;\n\npublic class UseB {\n    public void run(Node other) {\n        Lib.take(other);\n    }\n}\n",
                ),
                (
                    "src/c/UseA.java",
                    "package c;\n\nimport a.Node;\n\npublic class UseA {\n    public void run(Node same) {\n        Lib.take(same);\n    }\n}\n",
                ),
            ],
            "take",
            "src/c/Lib.java",
            "src/c/UseB.java",
            "src/c/UseA.java",
            "arguments `(Node)` bind only `take(Node)`",
        ),
        (
            vec![
                ("src/a/Node.kt", "package a\n\nclass Node\n"),
                ("src/b/Node.kt", "package b\n\nclass Node\n"),
                (
                    "src/c/Lib.kt",
                    "package c\n\nimport a.Node\n\nobject Lib {\n    fun take(n: Node) {}\n    fun take(o: Any) {}\n}\n",
                ),
                (
                    "src/c/UseB.kt",
                    "package c\n\nimport b.Node\n\nfun runB(other: Node) {\n    Lib.take(other)\n}\n",
                ),
                (
                    "src/c/UseA.kt",
                    "package c\n\nimport a.Node\n\nfun runA(same: Node) {\n    Lib.take(same)\n}\n",
                ),
            ],
            "take",
            "src/c/Lib.kt",
            "src/c/UseB.kt",
            "src/c/UseA.kt",
            "arguments `(Node)` bind only `take(Node)`",
        ),
        (
            vec![
                ("src/a/Node.scala", "package a\n\nclass Node\n"),
                ("src/b/Node.scala", "package b\n\nclass Node\n"),
                (
                    "src/c/Lib.scala",
                    "package c\n\nimport a.Node\n\nobject Lib {\n  def take(n: Node): Int = 1\n  def take(o: Any): Int = 2\n}\n",
                ),
                (
                    "src/c/UseB.scala",
                    "package c\n\nimport b.Node\n\nobject UseB {\n  def run(other: Node): Int = {\n    Lib.take(other)\n  }\n}\n",
                ),
                (
                    "src/c/UseA.scala",
                    "package c\n\nimport a.Node\n\nobject UseA {\n  def run(same: Node): Int = {\n    Lib.take(same)\n  }\n}\n",
                ),
            ],
            "take",
            "src/c/Lib.scala",
            "src/c/UseB.scala",
            "src/c/UseA.scala",
            "arguments `(Node)` bind only `take(Node)`",
        ),
        (
            vec![
                (
                    "src/Types.cs",
                    "namespace A { public class Node { } }\nnamespace B { public class Node { } }\n",
                ),
                (
                    "src/Lib.cs",
                    "using A;\n\nnamespace C\n{\n    public static class Lib\n    {\n        public static void Take(Node n) { }\n        public static void Take(object o) { }\n    }\n}\n",
                ),
                (
                    "src/UseB.cs",
                    "using B;\n\nnamespace C\n{\n    public class UseB\n    {\n        public void Run(Node other)\n        {\n            Lib.Take(other);\n        }\n    }\n}\n",
                ),
                (
                    "src/UseA.cs",
                    "using A;\n\nnamespace C\n{\n    public class UseA\n    {\n        public void Run(Node same)\n        {\n            Lib.Take(same);\n        }\n    }\n}\n",
                ),
            ],
            "Take",
            "src/Lib.cs",
            "src/UseB.cs",
            "src/UseA.cs",
            "arguments `(Node)` bind only `Take(Node)`",
        ),
    ];
    for (files, name, lib, use_other, use_same, basis) in cases {
        let repo = scanned_repo(&files);
        let by_node = report_of(&repo, name, lib, 0);
        let by_object = report_of(&repo, name, lib, 1);
        let other = line_of(&repo, use_other, &format!("Lib.{name}(other)"));
        assert_kept(&by_node, use_other, other);
        assert_kept(&by_object, use_other, other);
        let same = line_of(&repo, use_same, &format!("Lib.{name}(same)"));
        assert_kept(&by_node, use_same, same);
        assert_excluded(&by_object, use_same, same, basis);
    }
}

#[test]
fn a_simple_name_that_two_project_types_share_keeps_the_overloads_in_cpp_and_swift() {
    let cpp = scanned_repo(&[(
        "src/ov.cpp",
        "namespace a { struct Node {}; }\nnamespace b { struct Node {}; }\nnamespace lib {\nusing a::Node;\nint take(Node n) { return 1; }\ntemplate <typename T> int take(T t) { return 2; }\n}\nusing namespace b;\nint run() {\n    Node other;\n    return lib::take(other);\n}\n",
    )]);
    let call = line_of(&cpp, "src/ov.cpp", "lib::take(other)");
    for nth in [0, 1] {
        assert_kept(
            &report_of(&cpp, "take", "src/ov.cpp", nth),
            "src/ov.cpp",
            call,
        );
    }

    let swift = scanned_repo(&[
        ("Sources/A/Node.swift", "public struct Node {}\n"),
        (
            "Sources/B/Node.swift",
            "public struct Node {}\npublic func take(_ n: Node) -> Int { return 1 }\npublic func take(_ a: Any) -> Int { return 2 }\n",
        ),
        (
            "Sources/C/Use.swift",
            "import A\nimport B\nfunc run(other: A.Node) -> Int {\n    let copy: Node = other\n    return take(copy)\n}\n",
        ),
    ]);
    let path = "Sources/C/Use.swift";
    let call = line_of(&swift, path, "take(copy)");
    for nth in [0, 1] {
        assert_kept(
            &report_of(&swift, "take", "Sources/B/Node.swift", nth),
            path,
            call,
        );
    }
}

#[test]
fn a_call_on_the_line_of_its_callers_definition_gets_the_overload_check() {
    let cases = [
        (
            "Api.cs",
            "Parse",
            "#nullable enable\npublic class Settings {}\npublic class Api {\n    public static string Parse(string json) { return Parse(json, null); }\n    public static string Parse(string json, Settings? settings) { return json; }\n}\n",
            "arguments `(string, ?)` bind only `Parse(string, Settings?)`",
        ),
        (
            "src/p/Ov.java",
            "one",
            "package p;\n\nclass Ov {\n    static int one(int a) { return one(\"x\"); }\n    static int one(String s) { return 1; }\n}\n",
            "arguments `(String)` bind only `one(String)`",
        ),
        (
            "src/ov.kt",
            "one",
            "fun one(a: Int): Int = one(\"x\")\nfun one(s: String): Int = 1\n",
            "arguments `(String)` bind only `one(String)`",
        ),
        (
            "src/Ov.scala",
            "one",
            "object Ov {\n  def one(a: Int): Int = one(\"x\")\n  def one(s: String): Int = 1\n}\n",
            "arguments `(String)` bind only `one(String)`",
        ),
        (
            "src/ov.cpp",
            "one",
            "int one(int a) { return one(\"x\"); }\nint one(const char* s) { return 1; }\n",
            "arguments `(const char*)` bind only `one(const char*)`",
        ),
        (
            "src/ov.swift",
            "one",
            "class Ov {\n    func one(_ a: Int) -> Int { return one(\"x\") }\n    func one(_ s: String) -> Int { return 1 }\n}\n",
            "arguments `(String)` bind only `one(_: String)`",
        ),
    ];
    for (path, name, source, basis) in cases {
        let repo = scanned_repo(&[(path, source)]);
        let first = report_of(&repo, name, path, 0);
        let line = first
            .sites
            .first()
            .map_or(0, |site| site.site.start_line.unwrap_or(0));
        assert_excluded(&first, path, line, basis);
        assert_kept(&report_of(&repo, name, path, 1), path, line);
    }
}

#[test]
fn a_swift_optional_that_julie_writes_without_its_marker_keeps_the_overload_that_takes_it() {
    let path = "src/opt.swift";
    let repo = scanned_repo(&[(
        path,
        "class Opt {\n    var stored: Int! = nil\n    func nul(_ a: Int) -> Int { return 1 }\n    func nul(_ a: Int?) -> Int { return 2 }\n    func name(_ a: String) -> Int { return 1 }\n    func name(_ a: Int) -> Int { return 2 }\n    func go(n: Int!, m: Int?) {\n        _ = nul(n)\n        _ = nul(stored)\n        _ = nul(m)\n        _ = name(1)\n    }\n}\n",
    )]);
    let by_value = report_of(&repo, "nul", path, 0);
    let by_optional = report_of(&repo, "nul", path, 1);
    for line in [8, 9] {
        assert_kept(&by_value, path, line);
        assert_kept(&by_optional, path, line);
    }
    assert_kept(&by_optional, path, 10);
    assert_excluded(
        &by_value,
        path,
        10,
        "arguments `(Int?)` bind only `nul(_: Int?)`",
    );
    assert_excluded(
        &report_of(&repo, "name", path, 0),
        path,
        11,
        "arguments `(Int)` bind only `name(_: Int)`",
    );
}

#[test]
fn a_java_varargs_call_keeps_the_widening_overload_and_excludes_what_no_conversion_reaches() {
    let path = "src/p/Sum.java";
    let repo = scanned_repo(&[(
        path,
        "package p;\n\nclass Sum {\n    int sum(int... xs) { return 0; }\n    int sum(String s) { return 1; }\n    int sum(long a, long b) { return 2; }\n    int go() {\n        int one = sum(1);\n        int two = sum(1, 2);\n        int none = sum();\n        return one + two + none;\n    }\n}\n",
    )]);
    let varargs = report_of(&repo, "sum", path, 0);
    let by_string = report_of(&repo, "sum", path, 1);
    let by_long = report_of(&repo, "sum", path, 2);
    let line = |needle| line_of(&repo, path, needle);
    for call in ["sum(1);", "sum(1, 2)", "sum();"] {
        assert_kept(&varargs, path, line(call));
        assert_eq!(
            site(&by_string, path, line(call)).0,
            SiteLabel::Excluded,
            "{}",
            dump(&by_string)
        );
    }
    assert_kept(&by_long, path, line("sum(1, 2)"));
    for call in ["sum(1);", "sum();"] {
        assert_eq!(
            site(&by_long, path, line(call)).0,
            SiteLabel::Excluded,
            "{}",
            dump(&by_long)
        );
    }
}

#[test]
fn a_cpp_swift_or_vbnet_call_bound_to_another_overload_is_no_site_of_a_changed_overload() {
    let cases = [
        (
            "src/picker.cpp",
            "int pick(int v) { return v; }\nint pick(const char* v) { return 1; }\n",
            "int pick(int v, int w) { return v; }\nint pick(const char* v) { return 1; }\n",
            "src/use.cpp",
            "int byInt() { return pick(1); }\nint byString() { return pick(\"x\"); }\n",
        ),
        (
            "src/Picker.swift",
            "class Picker {\n    func pick(_ v: Int) {}\n    func pick(_ v: String) {}\n}\n",
            "class Picker {\n    func pick(_ v: Int, _ w: Int) {}\n    func pick(_ v: String) {}\n}\n",
            "src/Use.swift",
            "func byInt(p: Picker) { p.pick(1) }\nfunc byString(p: Picker) { p.pick(\"x\") }\n",
        ),
        (
            "src/Picker.vb",
            "Public Class Picker\n    Public Sub Pick(v As Integer)\n    End Sub\n    Public Sub Pick(v As String)\n    End Sub\nEnd Class\n",
            "Public Class Picker\n    Public Sub Pick(v As Integer, w As Integer)\n    End Sub\n    Public Sub Pick(v As String)\n    End Sub\nEnd Class\n",
            "src/Use.vb",
            "Public Module Use\n    Public Sub ByInt(p As Picker)\n        p.Pick(1)\n    End Sub\n    Public Sub ByString(p As Picker)\n        p.Pick(\"x\")\n    End Sub\nEnd Module\n",
        ),
    ];
    for (path, before, after, use_path, use_source) in cases {
        let repo = scanned_repo(&[(path, before), (use_path, use_source)]);
        write(&repo.root, path, after);
        let text = check_text(&repo);
        let (by_int, by_string) = if path.ends_with(".vb") {
            ("`ByInt`", "`ByString`")
        } else {
            ("`byInt`", "`byString`")
        };
        assert!(text.contains(by_int), "{path}: {text}");
        assert!(!text.contains(by_string), "{path}: {text}");
    }
}

#[test]
fn a_simple_name_that_reaches_a_library_type_never_names_the_project_type_or_another_one() {
    let java = scanned_repo(&[
        (
            "src/s/Svc.java",
            "package s;\n\nimport java.sql.Date;\n\npublic class Svc {\n    public static void put(Date d) {}\n    public static void put(Object o) {}\n    public static void say(String s) {}\n    public static void say(Object o) {}\n}\n",
        ),
        (
            "src/q/Process.java",
            "package q;\n\npublic class Process {}\n",
        ),
        (
            "src/q/Runner.java",
            "package q;\n\npublic class Runner {\n    public static void run(Process p) {}\n    public static void run(Object o) {}\n}\n",
        ),
        (
            "src/c/User.java",
            "package c;\n\nimport java.util.*;\nimport java.util.Date;\nimport q.Runner;\nimport s.Svc;\n\npublic class User {\n    void go(String text) throws Exception {\n        Date d = new Date();\n        Svc.put(d);\n        Process p = Runtime.getRuntime().exec(\"ls\");\n        Runner.run(p);\n        Svc.say(text);\n    }\n}\n",
        ),
    ]);
    let user = "src/c/User.java";
    let line = |needle| line_of(&java, user, needle);
    assert_kept(
        &report_of(&java, "put", "src/s/Svc.java", 1),
        user,
        line("Svc.put(d)"),
    );
    assert_kept(
        &report_of(&java, "run", "src/q/Runner.java", 1),
        user,
        line("Runner.run(p)"),
    );
    assert_excluded(
        &report_of(&java, "say", "src/s/Svc.java", 1),
        user,
        line("Svc.say(text)"),
        "arguments `(String)` bind only `say(String)`",
    );

    let kotlin = scanned_repo(&[
        (
            "src/s/Svc.kt",
            "package s\n\nimport java.sql.Date\n\nobject Svc {\n    fun put(d: Date) {}\n    fun put(o: Any) {}\n}\n",
        ),
        (
            "src/c/User.kt",
            "package c\n\nimport java.util.Date\nimport s.Svc\n\nclass User {\n    fun run() {\n        val d: Date = Date()\n        Svc.put(d)\n    }\n}\n",
        ),
    ]);
    assert_kept(
        &report_of(&kotlin, "put", "src/s/Svc.kt", 1),
        "src/c/User.kt",
        line_of(&kotlin, "src/c/User.kt", "Svc.put(d)"),
    );

    let csharp = scanned_repo(&[
        (
            "src/Err.cs",
            "using System;\nusing System.Text.Json;\n\nnamespace S\n{\n    public static class Err\n    {\n        public static void Handle(JsonException e) { }\n        public static void Handle(Exception e) { }\n    }\n}\n",
        ),
        (
            "src/Clock.cs",
            "namespace App\n{\n    public class Timer { }\n    public static class Clock\n    {\n        public static void Watch(Timer t) { }\n        public static void Watch(object o) { }\n    }\n}\n",
        ),
        (
            "src/User.cs",
            "using System;\nusing System.Threading;\nusing Newtonsoft.Json;\nusing S;\n\nnamespace Other\n{\n    public class User\n    {\n        public void Run(Exception error)\n        {\n            JsonException ex = new JsonException();\n            Err.Handle(ex);\n            Err.Handle(error);\n            Timer t = new Timer(null);\n            App.Clock.Watch(t);\n        }\n    }\n}\n",
        ),
    ]);
    let user = "src/User.cs";
    let line = |needle| line_of(&csharp, user, needle);
    assert_kept(
        &report_of(&csharp, "Handle", "src/Err.cs", 1),
        user,
        line("Err.Handle(ex)"),
    );
    assert_excluded(
        &report_of(&csharp, "Handle", "src/Err.cs", 0),
        user,
        line("Err.Handle(error)"),
        "arguments `(Exception)` bind only `Handle(Exception)`",
    );
    assert_kept(
        &report_of(&csharp, "Watch", "src/Clock.cs", 1),
        user,
        line("App.Clock.Watch(t)"),
    );

    let vbnet = scanned_repo(&[
        (
            "src/Err.vb",
            "Imports System\nImports System.Text.Json\n\nNamespace S\n    Public Module Err\n        Public Sub Handle(e As JsonException)\n        End Sub\n        Public Sub Handle(e As Exception)\n        End Sub\n    End Module\nEnd Namespace\n",
        ),
        (
            "src/User.vb",
            "Imports Newtonsoft.Json\nImports S\n\nNamespace C\n    Public Class User\n        Public Sub Go()\n            Dim ex As JsonException = New JsonException()\n            Err.Handle(ex)\n        End Sub\n    End Class\nEnd Namespace\n",
        ),
    ]);
    assert_kept(
        &report_of(&vbnet, "Handle", "src/Err.vb", 1),
        "src/User.vb",
        line_of(&vbnet, "src/User.vb", "Err.Handle(ex)"),
    );
}

#[test]
fn a_scala_exact_overload_that_needs_a_default_loses_to_one_that_applies_without_it() {
    let repo = scanned_repo(&[
        (
            "src/b/Svc.scala",
            "package b\n\nobject Svc {\n  def r(x: Any): Unit = {}\n  def r(x: Int, extra: Int = 0): Unit = {}\n}\n",
        ),
        (
            "src/b/User.scala",
            "package b\n\nobject User {\n  def other(): Unit = {\n    val i: Int = 3\n    Svc.r(i)\n  }\n}\n",
        ),
    ]);
    let user = "src/b/User.scala";
    let call = line_of(&repo, user, "Svc.r(i)");
    assert_kept(&report_of(&repo, "r", "src/b/Svc.scala", 0), user, call);
    assert_excluded(
        &report_of(&repo, "r", "src/b/Svc.scala", 1),
        user,
        call,
        "arguments `(Int)` bind only `r(Any)`",
    );
}

#[test]
fn a_scala_name_that_a_wildcard_import_and_the_package_both_reach_stays_unknown() {
    let files = |user: &'static str| {
        scanned_repo(&[
            ("src/a/Node.scala", "package a\n\nclass Node\n"),
            ("src/b/Node.scala", "package b\n\nclass Node\n"),
            (
                "src/b/Svc.scala",
                "package b\n\nobject Svc {\n  def take(n: b.Node): Unit = {}\n  def take(o: AnyRef): Unit = {}\n}\n",
            ),
            ("src/b/User.scala", user),
        ])
    };
    let user = "src/b/User.scala";
    let imported = files(
        "package b\n\nimport a._\n\nobject User {\n  def run(): Unit = {\n    val x: Node = new Node\n    Svc.take(x)\n  }\n}\n",
    );
    assert_kept(
        &report_of(&imported, "take", "src/b/Svc.scala", 1),
        user,
        line_of(&imported, user, "Svc.take(x)"),
    );
    let local = files(
        "package b\n\nobject User {\n  def run(): Unit = {\n    val x: Node = new Node\n    Svc.take(x)\n  }\n}\n",
    );
    assert_excluded(
        &report_of(&local, "take", "src/b/Svc.scala", 1),
        user,
        line_of(&local, user, "Svc.take(x)"),
        "arguments `(Node)` bind only `take(b.Node)`",
    );
}

#[test]
fn a_literal_a_new_object_or_a_plain_declared_value_is_no_hidden_nullable() {
    let csharp = scanned_repo(&[(
        "src/Svc.cs",
        "namespace App\n{\n    public struct Pt { public int X; }\n    public static class Svc\n    {\n        public static void Nul(int x) { }\n        public static void Nul(object x) { }\n        public static void Sp(Pt p) { }\n        public static void Sp(object o) { }\n    }\n    public class User\n    {\n        public void Run(int? n)\n        {\n            Svc.Nul(5);\n            Svc.Sp(new Pt());\n            int k = 3;\n            Svc.Nul(k);\n            Svc.Nul(k + 1);\n            Svc.Nul(n);\n        }\n    }\n}\n",
    )]);
    let path = "src/Svc.cs";
    let line = |needle| line_of(&csharp, path, needle);
    let nul_object = report_of(&csharp, "Nul", path, 1);
    for call in ["Svc.Nul(5)", "Svc.Nul(k);"] {
        assert_excluded(
            &nul_object,
            path,
            line(call),
            "arguments `(int)` bind only `Nul(int)`",
        );
    }
    for call in ["Svc.Nul(k + 1)", "Svc.Nul(n)"] {
        assert_kept(&nul_object, path, line(call));
    }
    assert_excluded(
        &report_of(&csharp, "Sp", path, 1),
        path,
        line("Svc.Sp(new Pt())"),
        "arguments `(Pt)` bind only `Sp(Pt)`",
    );

    let swift = scanned_repo(&[(
        "Sources/App/Svc.swift",
        "struct Pt { var x: Int }\nstruct Fl {\n    var x: Int\n    init?(s: String) { return nil }\n}\n\nenum Svc {\n    static func f(_ x: Int) {}\n    static func f(_ x: Any) {}\n    static func g(_ p: Pt) {}\n    static func g(_ p: Any) {}\n    static func h(_ p: Fl) {}\n    static func h(_ p: Any) {}\n}\n\nclass Holder {\n    var stored: Int! = nil\n    func run(w: Int!) {\n        Svc.f(5)\n        let v: Int = 1\n        Svc.f(v)\n        Svc.g(Pt(x: 1))\n        Svc.f(w)\n        Svc.f(stored)\n        let u = 3\n        Svc.f(u)\n        Svc.h(Fl(s: \"a\"))\n    }\n}\n",
    )]);
    let path = "Sources/App/Svc.swift";
    let line = |needle| line_of(&swift, path, needle);
    let f_any = report_of(&swift, "f", path, 1);
    for call in ["Svc.f(5)", "Svc.f(v)"] {
        assert_excluded(
            &f_any,
            path,
            line(call),
            "arguments `(Int)` bind only `f(_: Int)`",
        );
    }
    for call in ["Svc.f(w)", "Svc.f(stored)", "Svc.f(u)"] {
        assert_kept(&f_any, path, line(call));
    }
    assert_excluded(
        &report_of(&swift, "g", path, 1),
        path,
        line("Svc.g(Pt(x: 1))"),
        "arguments `(Pt)` bind only `g(_: Pt)`",
    );
    assert_kept(
        &report_of(&swift, "h", path, 1),
        path,
        line("Svc.h(Fl(s: \"a\"))"),
    );

    let kotlin = scanned_repo(&[(
        "src/Num.kt",
        "object Num {\n    fun f(x: Int) {}\n    fun f(x: Any?) {}\n}\n\nfun go(v: Int?) {\n    Num.f(5)\n    val w: Int? = v\n    Num.f(w)\n}\n",
    )]);
    let path = "src/Num.kt";
    let f_nullable = report_of(&kotlin, "f", path, 1);
    assert_excluded(
        &f_nullable,
        path,
        line_of(&kotlin, path, "Num.f(5)"),
        "arguments `(Int)` bind only `f(Int)`",
    );
    assert_kept(&f_nullable, path, line_of(&kotlin, path, "Num.f(w)"));
}

#[test]
fn a_kotlin_basis_names_a_nullable_overload_with_its_marker() {
    let repo = scanned_repo(&[
        (
            "src/s/Num.kt",
            "package s\n\nobject Num {\n    fun f(x: Int) {}\n    fun f(x: Long) {}\n    fun f(x: Any) {}\n    fun f(x: Any?) {}\n}\n",
        ),
        (
            "src/c/U2.kt",
            "package c\n\nimport s.Num\n\nclass U2 {\n    fun go(v: Int?) {\n        val w: Int? = v\n        Num.f(w)\n    }\n}\n",
        ),
    ]);
    let user = "src/c/U2.kt";
    assert_excluded(
        &report_of(&repo, "f", "src/s/Num.kt", 2),
        user,
        line_of(&repo, user, "Num.f(w)"),
        "arguments `(Int)` bind only `f(Int)` or `f(Any?)`",
    );
}
