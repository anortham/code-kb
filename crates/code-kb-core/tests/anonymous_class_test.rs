use code_kb_core::{
    BlastRadiusResult, ReferenceReport, SiteLabel, Workspace, blast_radius_op, edit_check_op,
    file_skeleton_op, find_julie_extract_binary, format_blast_radius, format_reference_report,
    get_symbol_by_id, lookup_symbols_op, open_read_only, reference_report, render_symbol_lookup,
    safe_tempdir, scan_workspace, select_reference_target, update_file,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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
    let target = select_reference_target(&conn, name, path)
        .unwrap_or_else(|error| panic!("{name} in {path:?}: {error}"));
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn dump(report: &ReferenceReport) -> String {
    let mut out = String::new();
    for site in &report.sites {
        out.push_str(&format!(
            "{} {}:{} {} from={} kind={}\n",
            site.label.as_str(),
            site.site.path,
            site.site.start_line.unwrap_or(0),
            site.basis,
            site.site.from_symbol_name,
            site.site.kind,
        ));
    }
    out.push_str(&format!(
        "overrides={:?}\noverridden={:?}\nconstructions={:?}\n",
        report.overrides, report.overridden, report.constructions
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

fn site_label(report: &ReferenceReport, path: &str, line: usize) -> (SiteLabel, String) {
    let site = report
        .sites
        .iter()
        .find(|site| site.site.path == path && site.site.start_line == Some(line))
        .unwrap_or_else(|| panic!("no site at {path}:{line}\n{}", dump(report)));
    (site.label, site.basis.clone())
}

fn walk(repo: &Repo, symbol: &str, path: &str) -> BlastRadiusResult {
    let conn = open_read_only(&repo.db).unwrap();
    let workspace = Workspace::new(repo.root.clone());
    blast_radius_op(&workspace, &conn, Some(symbol), Some(path), 3, 50).unwrap()
}

type ThisCall = (
    &'static str,
    &'static str,
    &'static str,
    (&'static str, &'static str),
);

struct Case {
    language: &'static str,
    files: &'static [(&'static str, &'static str)],
    path: &'static str,
    owner: &'static str,
    member: &'static str,
    base: (&'static str, &'static str),
    named: &'static str,
    outside: (&'static str, &'static str),
    this_call: Option<ThisCall>,
    test: Option<&'static str>,
}

impl Case {
    fn member_name(&self) -> String {
        format!("{}.{}", self.owner, self.member)
    }
}

const CASES: &[Case] = &[
    Case {
        language: "java",
        files: &[
            (
                "src/main/java/p/Task.java",
                "package p;\n\npublic abstract class Task {\n    public abstract int run();\n\n    public int go() {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/main/java/p/Impl.java",
                "package p;\n\npublic class Impl extends Task {\n    public int run() {\n        return this.go();\n    }\n}\n",
            ),
            (
                "src/main/java/p/Make.java",
                "package p;\n\npublic class Make {\n    public static Task make() {\n        return new Task() {\n            public int run() {\n                return this.go() + extra();\n            }\n\n            int extra() {\n                return 2;\n            }\n        };\n    }\n}\n",
            ),
            (
                "src/main/java/p/Use.java",
                "package p;\n\npublic class Use {\n    int use() {\n        return Make.make().run();\n    }\n}\n",
            ),
            (
                "src/test/java/p/MakeTest.java",
                "package p;\n\nimport org.junit.jupiter.api.Test;\n\nclass MakeTest {\n    @Test\n    void makesTask() {\n        Make.make();\n    }\n}\n",
            ),
        ],
        path: "src/main/java/p/Make.java",
        owner: "<anonymous Task in make>",
        member: "run",
        base: ("src/main/java/p/Task.java", "Task"),
        named: "Impl",
        outside: ("src/main/java/p/Use.java", "Make.make().run()"),
        this_call: Some((
            "src/main/java/p/Task.java",
            "go",
            "this.go()",
            ("src/main/java/p/Impl.java", "this.go()"),
        )),
        test: Some("makesTask"),
    },
    Case {
        language: "kotlin",
        files: &[
            (
                "src/main/kotlin/p/Task.kt",
                "package p\n\ninterface Task {\n    fun run(): Int\n\n    fun go(): Int = 1\n}\n",
            ),
            (
                "src/main/kotlin/p/Impl.kt",
                "package p\n\nclass Impl : Task {\n    override fun run(): Int = this.go()\n}\n",
            ),
            (
                "src/main/kotlin/p/Make.kt",
                "package p\n\nfun make(): Task {\n    return object : Task {\n        override fun run(): Int = this.go() + extra()\n\n        fun extra(): Int = 2\n    }\n}\n",
            ),
            (
                "src/main/kotlin/p/Use.kt",
                "package p\n\nfun use(): Int = make().run()\n",
            ),
            (
                "src/test/kotlin/p/MakeTest.kt",
                "package p\n\nimport org.junit.jupiter.api.Test\n\nclass MakeTest {\n    @Test\n    fun makesTask() {\n        make()\n    }\n}\n",
            ),
        ],
        path: "src/main/kotlin/p/Make.kt",
        owner: "<anonymous Task in make>",
        member: "run",
        base: ("src/main/kotlin/p/Task.kt", "Task"),
        named: "Impl",
        outside: ("src/main/kotlin/p/Use.kt", "make().run()"),
        this_call: Some((
            "src/main/kotlin/p/Task.kt",
            "go",
            "this.go()",
            ("src/main/kotlin/p/Impl.kt", "this.go()"),
        )),
        test: Some("makesTask"),
    },
    Case {
        language: "scala",
        files: &[
            (
                "src/main/scala/p/Task.scala",
                "package p\n\ntrait Task {\n  def run(): Int\n\n  def go(): Int = 1\n}\n",
            ),
            (
                "src/main/scala/p/Impl.scala",
                "package p\n\nclass Impl extends Task {\n  def run(): Int = this.go()\n}\n",
            ),
            (
                "src/main/scala/p/Make.scala",
                "package p\n\nobject Make {\n  def make(): Task = new Task {\n    def run(): Int = this.go() + extra()\n\n    def extra(): Int = 2\n  }\n}\n",
            ),
            (
                "src/main/scala/p/Use.scala",
                "package p\n\nobject Use {\n  def use(): Int = Make.make().run()\n}\n",
            ),
            (
                "src/test/scala/p/MakeTest.scala",
                "package p\n\nimport org.junit.Test\n\nclass MakeTest {\n  @Test\n  def makesTask(): Unit = {\n    Make.make()\n  }\n}\n",
            ),
        ],
        path: "src/main/scala/p/Make.scala",
        owner: "<anonymous Task in make>",
        member: "run",
        base: ("src/main/scala/p/Task.scala", "Task"),
        named: "Impl",
        outside: ("src/main/scala/p/Use.scala", "Make.make().run()"),
        this_call: Some((
            "src/main/scala/p/Task.scala",
            "go",
            "this.go()",
            ("src/main/scala/p/Impl.scala", "this.go()"),
        )),
        test: Some("makesTask"),
    },
    Case {
        language: "typescript",
        files: &[
            (
                "src/task.ts",
                "export interface Task {\n  run(): number;\n}\n\nexport class Base {\n  go(): number {\n    return 1;\n  }\n}\n",
            ),
            (
                "src/impl.ts",
                "import { Base, Task } from \"./task\";\n\nexport class Impl extends Base implements Task {\n  run(): number {\n    return this.go();\n  }\n}\n",
            ),
            (
                "src/make.ts",
                "import { Base, Task } from \"./task\";\n\nexport function make(): Task {\n  return new (class extends Base implements Task {\n    run(): number {\n      return this.go() + this.extra();\n    }\n\n    extra(): number {\n      return 2;\n    }\n  })();\n}\n",
            ),
            (
                "src/use.ts",
                "import { make } from \"./make\";\n\nexport function use(): number {\n  return make().run();\n}\n",
            ),
            (
                "test/make.test.ts",
                "import { make } from \"../src/make\";\n\ntest(\"builds a task\", () => {\n  make();\n});\n",
            ),
        ],
        path: "src/make.ts",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/task.ts", "Task"),
        named: "Impl",
        outside: ("src/use.ts", "make().run()"),
        this_call: Some((
            "src/task.ts",
            "go",
            "this.go()",
            ("src/impl.ts", "this.go()"),
        )),
        test: Some("builds a task"),
    },
    Case {
        language: "tsx",
        files: &[
            (
                "src/task.tsx",
                "export interface Task {\n  run(): number;\n}\n\nexport class Base {\n  go(): number {\n    return 1;\n  }\n}\n",
            ),
            (
                "src/impl.tsx",
                "import { Base, Task } from \"./task\";\n\nexport class Impl extends Base implements Task {\n  run(): number {\n    return this.go();\n  }\n}\n",
            ),
            (
                "src/make.tsx",
                "import { Base, Task } from \"./task\";\n\nexport function make(): Task {\n  return new (class extends Base implements Task {\n    run(): number {\n      return this.go() + this.extra();\n    }\n\n    extra(): number {\n      return 2;\n    }\n  })();\n}\n",
            ),
            (
                "src/use.tsx",
                "import { make } from \"./make\";\n\nexport function use(): number {\n  return make().run();\n}\n",
            ),
            (
                "test/make.test.tsx",
                "import { make } from \"../src/make\";\n\ntest(\"builds a task\", () => {\n  make();\n});\n",
            ),
        ],
        path: "src/make.tsx",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/task.tsx", "Task"),
        named: "Impl",
        outside: ("src/use.tsx", "make().run()"),
        this_call: Some((
            "src/task.tsx",
            "go",
            "this.go()",
            ("src/impl.tsx", "this.go()"),
        )),
        test: Some("builds a task"),
    },
    Case {
        language: "javascript",
        files: &[
            (
                "src/base.js",
                "export class Base {\n  run() {\n    return 0;\n  }\n\n  go() {\n    return 1;\n  }\n}\n",
            ),
            (
                "src/impl.js",
                "import { Base } from \"./base\";\n\nexport class Impl extends Base {\n  run() {\n    return this.go();\n  }\n}\n",
            ),
            (
                "src/make.js",
                "import { Base } from \"./base\";\n\nexport function make() {\n  return new (class extends Base {\n    run() {\n      return this.go() + this.extra();\n    }\n\n    extra() {\n      return 2;\n    }\n  })();\n}\n",
            ),
            (
                "src/use.js",
                "import { make } from \"./make\";\n\nexport function use() {\n  return make().run();\n}\n",
            ),
            (
                "test/make.test.js",
                "import { make } from \"../src/make\";\n\ntest(\"builds a task\", () => {\n  make();\n});\n",
            ),
        ],
        path: "src/make.js",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/base.js", "Base"),
        named: "Impl",
        outside: ("src/use.js", "make().run()"),
        this_call: Some((
            "src/base.js",
            "go",
            "this.go()",
            ("src/impl.js", "this.go()"),
        )),
        test: Some("builds a task"),
    },
    Case {
        language: "jsx",
        files: &[
            (
                "src/base.jsx",
                "export class Base {\n  run() {\n    return 0;\n  }\n\n  go() {\n    return 1;\n  }\n}\n",
            ),
            (
                "src/impl.jsx",
                "import { Base } from \"./base\";\n\nexport class Impl extends Base {\n  run() {\n    return this.go();\n  }\n}\n",
            ),
            (
                "src/make.jsx",
                "import { Base } from \"./base\";\n\nexport function make() {\n  return new (class extends Base {\n    run() {\n      return this.go() + this.extra();\n    }\n\n    extra() {\n      return 2;\n    }\n  })();\n}\n",
            ),
            (
                "src/use.jsx",
                "import { make } from \"./make\";\n\nexport function use() {\n  return make().run();\n}\n",
            ),
            (
                "test/make.test.jsx",
                "import { make } from \"../src/make\";\n\ntest(\"builds a task\", () => {\n  make();\n});\n",
            ),
        ],
        path: "src/make.jsx",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/base.jsx", "Base"),
        named: "Impl",
        outside: ("src/use.jsx", "make().run()"),
        this_call: Some((
            "src/base.jsx",
            "go",
            "this.go()",
            ("src/impl.jsx", "this.go()"),
        )),
        test: Some("builds a task"),
    },
    Case {
        language: "vue",
        files: &[
            (
                "src/base.js",
                "export class Base {\n  run() {\n    return 0;\n  }\n\n  go() {\n    return 1;\n  }\n}\n",
            ),
            (
                "src/impl.js",
                "import { Base } from \"./base\";\n\nexport class Impl extends Base {\n  run() {\n    return this.go();\n  }\n}\n",
            ),
            (
                "src/Make.vue",
                "<script>\nimport { Base } from \"./base\";\n\nexport function make() {\n  return new (class extends Base {\n    run() {\n      return this.go() + this.extra();\n    }\n\n    extra() {\n      return 2;\n    }\n  })();\n}\n</script>\n",
            ),
            (
                "src/use.js",
                "import { make } from \"./Make.vue\";\n\nexport function use() {\n  return make().run();\n}\n",
            ),
            (
                "test/make.test.js",
                "import { make } from \"../src/Make.vue\";\n\ntest(\"builds a task\", () => {\n  make();\n});\n",
            ),
        ],
        path: "src/Make.vue",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/base.js", "Base"),
        named: "Impl",
        outside: ("src/use.js", "make().run()"),
        this_call: Some((
            "src/base.js",
            "go",
            "this.go()",
            ("src/impl.js", "this.go()"),
        )),
        test: Some("builds a task"),
    },
    Case {
        language: "fsharp",
        files: &[
            (
                "src/Tasks.fs",
                "module Tasks\n\ntype ITask =\n    abstract Run : unit -> int\n",
            ),
            (
                "src/Impl.fs",
                "module Impl\n\nopen Tasks\n\ntype Impl() =\n    interface ITask with\n        member this.Run() = 2\n",
            ),
            (
                "src/Make.fs",
                "module Make\n\nopen Tasks\n\nlet make () =\n    { new ITask with\n        member this.Run() = 1 }\n",
            ),
            (
                "src/Use.fs",
                "module Use\n\nopen Make\n\nlet useIt () = (make ()).Run()\n",
            ),
            (
                "tests/MakeTests.fs",
                "module MakeTests\n\nopen Xunit\nopen Make\n\n[<Fact>]\nlet ``make builds a task`` () =\n    make () |> ignore\n",
            ),
        ],
        path: "src/Make.fs",
        owner: "<anonymous ITask in make>",
        member: "Run",
        base: ("src/Tasks.fs", "ITask"),
        named: "Impl",
        outside: ("src/Use.fs", "(make ()).Run()"),
        this_call: None,
        test: Some("``make builds a task``"),
    },
    Case {
        language: "ruby",
        files: &[
            (
                "lib/base.rb",
                "class Base\n  def run\n    0\n  end\n\n  def go\n    1\n  end\nend\n",
            ),
            (
                "lib/impl.rb",
                "require_relative \"base\"\n\nclass Impl < Base\n  def run\n    go\n  end\nend\n",
            ),
            (
                "lib/make.rb",
                "require_relative \"base\"\n\ndef make\n  Class.new(Base) do\n    def run\n      go + extra\n    end\n\n    def extra\n      2\n    end\n  end\nend\n",
            ),
            (
                "lib/use.rb",
                "require_relative \"make\"\n\ndef use_it\n  make.new.run\nend\n",
            ),
            (
                "test/make_test.rb",
                "require \"minitest/autorun\"\nrequire_relative \"../lib/make\"\n\nclass MakeTest < Minitest::Test\n  def test_make\n    make\n  end\nend\n",
            ),
        ],
        path: "lib/make.rb",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("lib/base.rb", "Base"),
        named: "Impl",
        outside: ("lib/use.rb", "make.new.run"),
        this_call: Some(("lib/base.rb", "go", "go + extra", ("lib/impl.rb", "    go"))),
        test: Some("test_make"),
    },
    Case {
        language: "php",
        files: &[
            (
                "src/Base.php",
                "<?php\nabstract class Base {\n    abstract public function run();\n\n    public function go() {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/Impl.php",
                "<?php\nclass Impl extends Base {\n    public function run() {\n        return $this->go();\n    }\n}\n",
            ),
            (
                "src/make.php",
                "<?php\nfunction make() {\n    return new class extends Base {\n        public function run() {\n            return $this->go() + $this->extra();\n        }\n\n        public function extra() {\n            return 2;\n        }\n    };\n}\n",
            ),
            (
                "src/use.php",
                "<?php\nfunction use_it() {\n    return make()->run();\n}\n",
            ),
            (
                "tests/MakeTest.php",
                "<?php\nuse PHPUnit\\Framework\\TestCase;\n\nclass MakeTest extends TestCase {\n    public function testMake() {\n        make();\n    }\n}\n",
            ),
        ],
        path: "src/make.php",
        owner: "<anonymous Base in make>",
        member: "run",
        base: ("src/Base.php", "Base"),
        named: "Impl",
        outside: ("src/use.php", "make()->run()"),
        this_call: Some((
            "src/Base.php",
            "go",
            "$this->go()",
            ("src/Impl.php", "$this->go()"),
        )),
        test: Some("testMake"),
    },
];

#[test]
fn an_anonymous_class_is_named_by_the_type_it_creates_and_the_symbol_that_creates_it() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        let conn = open_read_only(&repo.db).unwrap();

        let member = select_reference_target(&conn, &case.member_name(), None)
            .unwrap_or_else(|error| panic!("{}: {error}", case.language));
        let owner = get_symbol_by_id(&conn, member.parent_symbol_id.as_deref().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                owner.name.as_str(),
                owner.kind.as_str(),
                owner.path.as_str()
            ),
            (case.owner, "class", case.path),
            "{}",
            case.language
        );

        let lookup =
            lookup_symbols_op(&repo.db, &conn, case.member, None, None, false, 20).unwrap();
        let text = render_symbol_lookup(&conn, case.member, &lookup, 20).unwrap();
        assert!(
            text.contains(&format!("`{}`", case.member_name())),
            "{}: {text}",
            case.language
        );

        for query in ["anonymous", "anonymous_class", "anonymous class"] {
            let lookup = lookup_symbols_op(&repo.db, &conn, query, None, None, false, 20).unwrap();
            assert!(
                lookup
                    .names
                    .iter()
                    .chain(lookup.ranked.iter().map(|row| &row.symbol))
                    .all(|symbol| symbol.kind != "class" || !symbol.name.starts_with('<')),
                "{}: {query} lists an anonymous class: {:?}",
                case.language,
                lookup.names
            );
        }
        let exact = lookup_symbols_op(&repo.db, &conn, case.owner, None, None, false, 20).unwrap();
        assert_eq!(
            exact
                .names
                .iter()
                .map(|symbol| symbol.name.as_str())
                .collect::<Vec<_>>(),
            [case.owner],
            "{}",
            case.language
        );
    }
}

#[test]
fn an_anonymous_class_member_overrides_the_member_of_its_base_like_a_named_class() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        let base = report(
            &repo,
            &format!("{}.{}", case.base.1, case.member),
            Some(case.base.0),
        );
        let overrides: Vec<(&str, &str)> = base
            .overrides
            .iter()
            .map(|row| (row.owner.as_str(), row.path.as_str()))
            .collect();
        assert!(
            overrides.contains(&(case.owner, case.path)),
            "{}: {overrides:?}\n{}",
            case.language,
            dump(&base)
        );
        assert!(
            overrides.iter().any(|(owner, _)| *owner == case.named),
            "{}: the named class is missing: {overrides:?}",
            case.language
        );

        let member = report(&repo, &case.member_name(), None);
        assert!(
            member
                .overridden
                .iter()
                .any(|row| row.owner == case.base.1 && row.path == case.base.0),
            "{}\n{}",
            case.language,
            dump(&member)
        );
    }
}

#[test]
fn a_member_of_an_anonymous_class_is_not_local_to_the_symbol_that_creates_it() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        let member = report(&repo, &case.member_name(), None);
        let line = line_of(&repo, case.outside.0, case.outside.1);
        let (_, basis) = site_label(&member, case.outside.0, line);

        assert!(
            !basis.contains("is local to"),
            "{}: {basis}\n{}",
            case.language,
            dump(&member)
        );
    }
}

#[test]
fn this_calls_in_an_anonymous_class_reach_its_ancestors_as_in_a_named_class() {
    for case in CASES {
        let Some((path, name, needle, named)) = case.this_call else {
            continue;
        };
        let repo = scanned_repo(case.files);
        let ancestor = report(&repo, name, Some(path));
        let anonymous = site_label(&ancestor, case.path, line_of(&repo, case.path, needle));
        let named = site_label(&ancestor, named.0, line_of(&repo, named.0, named.1));

        assert_ne!(
            anonymous.0,
            SiteLabel::Excluded,
            "{}\n{}",
            case.language,
            dump(&ancestor)
        );
        assert_eq!(anonymous, named, "{}\n{}", case.language, dump(&ancestor));
    }
}

#[test]
fn blast_radius_walks_from_an_anonymous_class_member_to_the_symbol_that_creates_it() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        let result = walk(&repo, &case.member_name(), case.path);
        let text = format_blast_radius(&result);

        let creates = format!("creates `{}`", case.owner);
        assert!(
            result
                .impacted_symbols
                .iter()
                .any(|row| row.name == "make" && row.reason.as_deref() == Some(creates.as_str())),
            "{}: {text}",
            case.language
        );
        assert!(
            case.test.is_none_or(|name| result
                .likely_tests
                .iter()
                .any(|test| test.name.ends_with(name))),
            "{}: {text}",
            case.language
        );
        assert!(
            text.contains("`make` [line ") && text.contains(&format!("({creates})\n")),
            "{}: {text}",
            case.language
        );
    }
}

#[test]
fn a_call_of_its_own_member_inside_an_anonymous_class_is_proven() {
    for case in CASES {
        let Some((_, _, needle, _)) = case.this_call else {
            continue;
        };
        let repo = scanned_repo(case.files);
        let own = report(&repo, &format!("{}.extra", case.owner), Some(case.path));
        let (label, basis) = site_label(&own, case.path, line_of(&repo, case.path, needle));

        assert_eq!(
            label,
            SiteLabel::Proven,
            "{}: {basis}\n{}",
            case.language,
            dump(&own)
        );
    }
}

#[test]
fn a_skeleton_names_the_anonymous_class_that_a_function_defines() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        let conn = open_read_only(&repo.db).unwrap();
        let workspace = Workspace::new(repo.root.clone());
        let skeleton = file_skeleton_op(&workspace, &repo.db, &conn, case.path).unwrap();

        assert!(
            skeleton.contains(&format!("`{}`", case.owner))
                && !skeleton.contains("anonymous_class_L"),
            "{}: {skeleton}",
            case.language
        );
    }
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

#[test]
fn a_line_shift_above_an_anonymous_class_changes_no_shape() {
    for case in CASES {
        let repo = scanned_repo(case.files);
        git(&repo.root, &["init", "-q", "-b", "main"]);
        git(&repo.root, &["add", "."]);
        git(&repo.root, &["commit", "-q", "-m", "base"]);
        let file = repo.root.join(case.path);
        let text = fs::read_to_string(&file).unwrap();
        fs::write(&file, format!("\n\n{text}")).unwrap();

        let conn = open_read_only(&repo.db).unwrap();
        let workspace = Workspace::new(repo.root.clone());
        let check = edit_check_op(&workspace, &repo.db, &conn, None).unwrap();

        assert!(
            check.changes.is_empty(),
            "{}: {:?}",
            case.language,
            check
                .changes
                .iter()
                .map(|change| (&change.display, &change.change))
                .collect::<Vec<_>>()
        );
    }
}

struct ConstructionCase {
    files: &'static [(&'static str, &'static str)],
    base: (&'static str, &'static str),
    path: &'static str,
    member: &'static str,
    needle: &'static str,
}

const CONSTRUCTION_CASES: &[ConstructionCase] = &[
    ConstructionCase {
        files: &[
            (
                "src/p/Base.java",
                "package p;\n\npublic class Base {\n    public Base(int n) {}\n\n    public int go() {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/p/Make.java",
                "package p;\n\npublic class Make {\n    public static Base make() {\n        return new Base(2) {\n            public int go() {\n                return 3;\n            }\n        };\n    }\n}\n",
            ),
        ],
        base: ("src/p/Base.java", "Base"),
        path: "src/p/Make.java",
        member: "<anonymous Base in make>.go",
        needle: "new Base(2)",
    },
    ConstructionCase {
        files: &[
            (
                "src/p/Base.kt",
                "package p\n\nopen class Base(val n: Int) {\n    open fun go(): Int = 1\n}\n",
            ),
            (
                "src/p/Make.kt",
                "package p\n\nfun make(): Base = object : Base(2) {\n    override fun go(): Int = 3\n}\n",
            ),
        ],
        base: ("src/p/Base.kt", "Base"),
        path: "src/p/Make.kt",
        member: "<anonymous Base in make>.go",
        needle: "object : Base(2)",
    },
    ConstructionCase {
        files: &[
            (
                "src/p/Base.scala",
                "package p\n\nclass Base(n: Int) {\n  def go(): Int = 1\n}\n",
            ),
            (
                "src/p/Make.scala",
                "package p\n\nobject Make {\n  def make(): Base = new Base(2) {\n    override def go(): Int = 3\n  }\n}\n",
            ),
        ],
        base: ("src/p/Base.scala", "Base"),
        path: "src/p/Make.scala",
        member: "<anonymous Base in make>.go",
        needle: "new Base(2)",
    },
    ConstructionCase {
        files: &[
            (
                "src/Base.fs",
                "module Shapes\n\ntype Base(n: int) =\n    abstract Go : unit -> int\n    default this.Go() = 1\n",
            ),
            (
                "src/Make.fs",
                "module Make\n\nopen Shapes\n\nlet make () =\n    { new Base(2) with\n        member this.Go() = 3 }\n",
            ),
        ],
        base: ("src/Base.fs", "Base"),
        path: "src/Make.fs",
        member: "<anonymous Base in make>.Go",
        needle: "new Base(2)",
    },
    ConstructionCase {
        files: &[
            (
                "src/Base.php",
                "<?php\nclass Base {\n    public function __construct($n) {}\n\n    public function go() {\n        return 1;\n    }\n}\n",
            ),
            (
                "src/make.php",
                "<?php\nfunction make() {\n    return new class(2) extends Base {\n        public function go() {\n            return 3;\n        }\n    };\n}\n",
            ),
        ],
        base: ("src/Base.php", "Base"),
        path: "src/make.php",
        member: "<anonymous Base in make>.go",
        needle: "new class(2) extends Base",
    },
];

#[test]
fn an_anonymous_class_that_extends_a_class_is_a_construction_of_it() {
    for case in CONSTRUCTION_CASES {
        let repo = scanned_repo(case.files);
        let base = report(&repo, case.base.1, Some(case.base.0));
        let line = line_of(&repo, case.path, case.needle);

        assert!(
            base.constructions
                .iter()
                .any(|row| row.path == case.path && row.line == line),
            "{}\n{}",
            case.path,
            dump(&base)
        );
        let text = format_reference_report(&base, 20);
        assert!(!text.contains("anonymous_class_L"), "{text}");
        let conn = open_read_only(&repo.db).unwrap();
        assert!(
            select_reference_target(&conn, case.member, Some(case.path)).is_ok(),
            "{}",
            case.path
        );
    }
}

#[test]
fn a_class_expression_or_ruby_class_new_builds_no_object_of_its_base() {
    let repo = scanned_repo(&[
        (
            "src/base.ts",
            "export class Base {\n  go(): number {\n    return 1;\n  }\n}\n",
        ),
        (
            "src/make.ts",
            "import { Base } from \"./base\";\n\nexport function make(register: (c: unknown) => void) {\n  register(class extends Base {\n    go(): number {\n      return 2;\n    }\n  });\n}\n",
        ),
        ("lib/base.rb", "class Base\n  def go\n    1\n  end\nend\n"),
        (
            "lib/make.rb",
            "require_relative \"base\"\n\ndef make\n  Class.new(Base) do\n    def go\n      2\n    end\n  end\nend\n",
        ),
    ]);

    for path in ["src/base.ts", "lib/base.rb"] {
        let base = report(&repo, "Base", Some(path));
        assert!(base.constructions.is_empty(), "{path}\n{}", dump(&base));
    }
}

#[test]
fn a_function_defined_in_the_initializer_of_a_value_is_local_to_it() {
    let repo = scanned_repo(&[
        (
            "src/p/Util.scala",
            "package p\n\nobject Util {\n  def helper(): Int = 1\n\n  def outside(): Int = {\n    Util.helper()\n  }\n}\n",
        ),
        (
            "src/p/Use.scala",
            "package p\n\nobject Use {\n  val inside: Int = {\n    def helper(): Int = 2\n    helper()\n  }\n}\n",
        ),
    ]);
    let local = report(&repo, "helper", Some("src/p/Use.scala"));
    let outside = line_of(&repo, "src/p/Util.scala", "Util.helper()");
    let inside = line_of(&repo, "src/p/Use.scala", "    helper()");

    assert_eq!(
        site_label(&local, "src/p/Util.scala", outside),
        (
            SiteLabel::Excluded,
            "`helper` is local to `inside`".to_string()
        ),
        "{}",
        dump(&local)
    );
    assert_ne!(
        site_label(&local, "src/p/Use.scala", inside).0,
        SiteLabel::Excluded,
        "{}",
        dump(&local)
    );
}

#[test]
fn a_kotlin_object_expression_member_without_override_is_not_local() {
    let repo = scanned_repo(&[(
        "src/p/Make.kt",
        "package p\n\nprivate fun make() = object {\n    fun extra(): Int = 2\n}\n\nfun use(): Int = make().extra()\n",
    )]);
    let extra = report(&repo, "<anonymous in make>.extra", None);
    let line = line_of(&repo, "src/p/Make.kt", "make().extra()");

    assert_ne!(
        site_label(&extra, "src/p/Make.kt", line).0,
        SiteLabel::Excluded,
        "{}",
        dump(&extra)
    );
}

struct InTestCase {
    files: &'static [(&'static str, &'static str)],
    target: (&'static str, &'static str),
    test: &'static str,
}

const IN_TEST_CASES: &[InTestCase] = &[
    InTestCase {
        files: &[
            (
                "src/main/java/p/Work.java",
                "package p;\n\npublic class Work {\n    public static void work() {}\n}\n",
            ),
            (
                "src/test/java/p/WorkTest.java",
                "package p;\n\nimport org.junit.jupiter.api.Test;\n\nclass WorkTest {\n    @Test\n    void runsWork() {\n        Runnable r = new Runnable() {\n            public void run() {\n                Work.work();\n            }\n        };\n        r.run();\n    }\n}\n",
            ),
        ],
        target: ("src/main/java/p/Work.java", "work"),
        test: "runsWork",
    },
    InTestCase {
        files: &[
            ("src/main/kotlin/p/Work.kt", "package p\n\nfun work() {}\n"),
            (
                "src/test/kotlin/p/WorkTest.kt",
                "package p\n\nimport org.junit.jupiter.api.Test\n\nclass WorkTest {\n    @Test\n    fun runsWork() {\n        val r = object : Runnable {\n            override fun run() {\n                work()\n            }\n        }\n        r.run()\n    }\n}\n",
            ),
        ],
        target: ("src/main/kotlin/p/Work.kt", "work"),
        test: "runsWork",
    },
    InTestCase {
        files: &[
            ("src/main/kotlin/p/Work.kt", "package p\n\nfun work() {}\n"),
            (
                "src/test/kotlin/p/WorkTest.kt",
                "package p\n\nimport org.junit.jupiter.api.Test\n\nclass WorkTest {\n    @Test\n    fun runsWork() {\n        val r = object : Runnable {\n            override fun run() {\n                testHelper()\n            }\n\n            fun testHelper() {\n                work()\n            }\n        }\n        r.run()\n    }\n}\n",
            ),
        ],
        target: ("src/main/kotlin/p/Work.kt", "work"),
        test: "runsWork",
    },
    InTestCase {
        files: &[
            (
                "src/main/scala/p/Work.scala",
                "package p\n\nobject Work {\n  def work(): Unit = ()\n}\n",
            ),
            (
                "src/test/scala/p/WorkTest.scala",
                "package p\n\nimport org.junit.Test\n\nclass WorkTest {\n  @Test\n  def runsWork(): Unit = {\n    val r = new Runnable {\n      def run(): Unit = Work.work()\n    }\n    r.run()\n  }\n}\n",
            ),
        ],
        target: ("src/main/scala/p/Work.scala", "work"),
        test: "runsWork",
    },
    InTestCase {
        files: &[
            ("src/work.ts", "export function work(): void {}\n"),
            (
                "test/work.test.ts",
                "import { work } from \"../src/work\";\n\ntest(\"runs work\", () => {\n  new (class {\n    run(): void {\n      work();\n    }\n  })().run();\n});\n",
            ),
        ],
        target: ("src/work.ts", "work"),
        test: "runs work",
    },
    InTestCase {
        files: &[
            ("lib/work.rb", "def work\n  1\nend\n"),
            (
                "test/work_test.rb",
                "require \"minitest/autorun\"\nrequire_relative \"../lib/work\"\n\nclass WorkTest < Minitest::Test\n  def test_work\n    Class.new do\n      def run\n        work\n      end\n    end.new.run\n  end\nend\n",
            ),
        ],
        target: ("lib/work.rb", "work"),
        test: "test_work",
    },
    InTestCase {
        files: &[
            (
                "src/work.php",
                "<?php\nfunction work() {\n    return 1;\n}\n",
            ),
            (
                "tests/WorkTest.php",
                "<?php\nuse PHPUnit\\Framework\\TestCase;\n\nclass WorkTest extends TestCase {\n    public function testWork() {\n        $r = new class {\n            public function run() {\n                return work();\n            }\n        };\n        $r->run();\n    }\n}\n",
            ),
        ],
        target: ("src/work.php", "work"),
        test: "testWork",
    },
];

#[test]
fn a_call_in_an_anonymous_class_inside_a_test_lists_that_test() {
    for case in IN_TEST_CASES {
        let repo = scanned_repo(case.files);
        let result = walk(&repo, case.target.1, case.target.0);

        assert!(
            result
                .likely_tests
                .iter()
                .any(|test| test.name.ends_with(case.test)),
            "{}: {}",
            case.target.0,
            format_blast_radius(&result)
        );
        assert!(
            result
                .likely_tests
                .iter()
                .all(|test| !test.name.contains("<anonymous") && !test.name.ends_with("testHelper")),
            "{}: {}",
            case.target.0,
            format_blast_radius(&result)
        );
        assert!(
            result
                .impacted_symbols
                .iter()
                .all(|row| !row.name.starts_with("<anonymous")),
            "{}: {}",
            case.target.0,
            format_blast_radius(&result)
        );

        let callers = report(&repo, case.target.1, Some(case.target.0));
        let tests: Vec<(&str, Option<&str>)> = callers
            .tests
            .iter()
            .map(|test| (test.name.as_str(), test.command.as_deref()))
            .collect();
        assert!(
            tests.iter().any(|(name, _)| name.ends_with(case.test))
                && tests.iter().all(|(name, command)| {
                    !name.ends_with("testHelper")
                        && command.is_none_or(|command| !command.contains("<anonymous"))
                }),
            "{}: {tests:?}\n{}",
            case.target.0,
            dump(&callers)
        );
    }
}

#[test]
fn removing_an_anonymous_class_reports_its_members_and_not_the_class() {
    for case in CASES
        .iter()
        .filter(|case| matches!(case.language, "java" | "kotlin"))
    {
        let repo = scanned_repo(case.files);
        git(&repo.root, &["init", "-q", "-b", "main"]);
        git(&repo.root, &["add", "."]);
        git(&repo.root, &["commit", "-q", "-m", "base"]);
        let replacement = if case.language == "java" {
            "package p;\n\npublic class Make {\n    public static Task make() {\n        return null;\n    }\n}\n"
        } else {
            "package p\n\nfun make(): Task {\n    throw IllegalStateException()\n}\n"
        };
        fs::write(repo.root.join(case.path), replacement).unwrap();

        let conn = open_read_only(&repo.db).unwrap();
        let workspace = Workspace::new(repo.root.clone());
        let check = edit_check_op(&workspace, &repo.db, &conn, None).unwrap();
        let displays: Vec<&str> = check
            .changes
            .iter()
            .map(|change| change.display.as_str())
            .collect();

        assert!(
            displays.contains(&case.member_name().as_str())
                && displays.contains(&format!("{}.extra", case.owner).as_str())
                && !displays.iter().any(|display| display.ends_with(case.owner)),
            "{}: {displays:?}",
            case.language
        );
    }
}

#[test]
fn an_anonymous_class_without_a_named_base_is_named_by_its_creator() {
    let repo = scanned_repo(&[
        (
            "lib/make.rb",
            "def make\n  Struct.new(:a) do\n    def twice\n      a * 2\n    end\n  end\nend\n",
        ),
        (
            "src/p/Make.kt",
            "package p\n\nfun make() = object {\n    fun x() = 1\n}\n\nfun hold() {\n    val r = object : Runnable {\n        override fun run() {}\n    }\n    r.run()\n}\n",
        ),
        (
            "src/make.js",
            "export function make() {\n  return new (class {\n    x() {\n      return 1;\n    }\n  })();\n}\n",
        ),
        (
            "src/make.php",
            "<?php\nfunction make() {\n    return new class {\n        public function x() {\n            return 1;\n        }\n    };\n}\n",
        ),
        (
            "src/p/Holder.java",
            "package p;\n\npublic class Holder {\n    Runnable field = new Runnable() {\n        public void run() {}\n    };\n}\n",
        ),
    ]);
    let conn = open_read_only(&repo.db).unwrap();
    let mut names: Vec<(String, String)> = conn
        .prepare(
            "SELECT path, name FROM symbols WHERE kind = 'class' AND name LIKE '<%' ORDER BY path",
        )
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    names.sort();

    assert_eq!(
        names,
        [
            ("lib/make.rb", "<anonymous Struct in make>"),
            ("src/make.js", "<anonymous in make>"),
            ("src/make.php", "<anonymous in make>"),
            ("src/p/Holder.java", "<anonymous Runnable in field>"),
            ("src/p/Make.kt", "<anonymous Runnable in r>"),
            ("src/p/Make.kt", "<anonymous in make>"),
        ]
        .map(|(path, name)| (path.to_string(), name.to_string()))
    );
}

#[test]
fn an_incremental_update_names_the_anonymous_classes_it_writes() {
    let case = &CASES[0];
    let repo = scanned_repo(case.files);
    let file = repo.root.join(case.path);
    let text = fs::read_to_string(&file).unwrap();
    fs::write(
        &file,
        text.replace("public static Task make()", "public static Task build()"),
    )
    .unwrap();

    update_file(&Workspace::new(repo.root.clone()), &repo.db, case.path).unwrap();

    let conn = open_read_only(&repo.db).unwrap();
    let names: Vec<String> = conn
        .prepare("SELECT name FROM symbols WHERE kind = 'class' AND path = ?1 ORDER BY start_line")
        .unwrap()
        .query_map([case.path], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(names, ["Make", "<anonymous Task in build>"]);
}

const INTERFACE_DEFAULT_CASES: &[(&str, &[(&str, &str)])] = &[
    (
        "src/main/java/p/Task.java",
        &[
            (
                "src/main/java/p/Task.java",
                "package p;\n\npublic interface Task {\n    default int go() {\n        return 1;\n    }\n\n    int run();\n}\n",
            ),
            (
                "src/main/java/p/Impl.java",
                "package p;\n\npublic class Impl implements Task {\n    public int run() {\n        int a = this.go();\n        int b = go();\n        return a + b;\n    }\n}\n",
            ),
            (
                "src/main/java/p/Make.java",
                "package p;\n\npublic class Make {\n    static Task make() {\n        return new Task() {\n            public int run() {\n                int a = this.go();\n                int b = go();\n                return a + b;\n            }\n        };\n    }\n}\n",
            ),
            (
                "src/main/java/p/Other.java",
                "package p;\n\npublic class Other {\n    public int go() {\n        return 2;\n    }\n}\n",
            ),
        ],
    ),
    (
        "src/main/kotlin/p/Task.kt",
        &[
            (
                "src/main/kotlin/p/Task.kt",
                "package p\n\ninterface Task {\n    fun go(): Int = 1\n\n    fun run(): Int\n}\n",
            ),
            (
                "src/main/kotlin/p/Impl.kt",
                "package p\n\nclass Impl : Task {\n    override fun run(): Int {\n        val a = this.go()\n        val b = go()\n        return a + b\n    }\n}\n",
            ),
            (
                "src/main/kotlin/p/Make.kt",
                "package p\n\nfun make(): Task {\n    return object : Task {\n        override fun run(): Int {\n                val a = this.go()\n                val b = go()\n                return a + b\n            }\n    }\n}\n",
            ),
            (
                "src/main/kotlin/p/Other.kt",
                "package p\n\nclass Other {\n    fun go(): Int = 2\n}\n",
            ),
        ],
    ),
];

#[test]
fn a_call_in_a_class_or_anonymous_object_reaches_a_default_method_of_its_interface() {
    for (interface, files) in INTERFACE_DEFAULT_CASES {
        let repo = scanned_repo(files);
        let go = report(&repo, "go", Some(interface));

        for (path, _) in &files[1..3] {
            for needle in ["this.go()", "= go()"] {
                let (label, basis) = site_label(&go, path, line_of(&repo, path, needle));
                assert_eq!(
                    (label, basis.as_str()),
                    (SiteLabel::Proven, "class chain"),
                    "{path} {needle}\n{}",
                    dump(&go)
                );
            }
        }
    }
}
