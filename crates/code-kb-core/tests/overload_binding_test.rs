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

#[test]
fn a_vbnet_object_argument_binds_late_and_keeps_every_overload_it_fits() {
    let path = "src/Late.vb";
    let repo = scanned_repo(&[(
        path,
        "Public Class Late\n    Public Function Run(a As Object) As Integer\n        Return 1\n    End Function\n    Public Function Run(a As String) As Integer\n        Return 2\n    End Function\n    Public Function Go(o As Object, s As String) As Integer\n        Run(o)\n        Run(s)\n        Run(Nothing)\n        Return 0\n    End Function\nEnd Class\n",
    )]);
    let by_object = report_of(&repo, "Run", path, 0);
    let by_string = report_of(&repo, "Run", path, 1);
    for line in [9, 11] {
        assert_kept(&by_object, path, line);
        assert_kept(&by_string, path, line);
    }
    assert_kept(&by_string, path, 10);
    assert_excluded(
        &by_object,
        path,
        10,
        "arguments `(String)` bind only `Run(String)`",
    );
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
