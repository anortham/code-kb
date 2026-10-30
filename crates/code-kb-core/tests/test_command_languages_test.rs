use code_kb_core::{
    ReferenceReport, Workspace, find_julie_extract_binary, open_read_only, reference_report,
    safe_tempdir, scan_workspace, select_reference_target,
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

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path)).unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn line_of(repo: &Repo, path: &str, needle: &str) -> usize {
    let text = fs::read_to_string(repo.root.join(path)).unwrap();
    text.lines()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("`{needle}` not in {path}"))
        + 1
}

fn commands(report: &ReferenceReport) -> Vec<(String, String)> {
    report
        .tests
        .iter()
        .map(|test| {
            (
                test.name.clone(),
                test.command.clone().unwrap_or_else(|| "-".to_string()),
            )
        })
        .collect()
}

fn assert_commands(report: &ReferenceReport, expected: &[(&str, &str)]) {
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(name, command)| (name.to_string(), command.to_string()))
        .collect();
    assert_eq!(commands(report), expected, "{report:#?}");
}

const KOTLIN_LIB: &str = "package demo\n\nfun target(): Int = 1\n";
const KOTLIN_TEST: &str = "package demo\n\nimport org.junit.jupiter.api.Test\n\nclass LibTest {\n    @Test fun addsTwo() { target() }\n\n    @Test fun `adds two numbers`() { target() }\n\n    class Inner {\n        @Test fun innerCase() { target() }\n    }\n}\n";

#[test]
fn kotlin_tests_run_through_the_gradle_wrapper_of_their_build() {
    let repo = scanned_repo(&[
        ("build.gradle.kts", "plugins { kotlin(\"jvm\") }\n"),
        ("gradlew", "#!/bin/sh\n"),
        ("gradlew.bat", "@echo off\n"),
        ("src/main/kotlin/demo/Lib.kt", KOTLIN_LIB),
        ("src/test/kotlin/demo/LibTest.kt", KOTLIN_TEST),
    ]);
    let tool = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "./gradlew"
    };

    let target = report(&repo, "target", "src/main/kotlin/demo/Lib.kt");

    assert_commands(
        &target,
        &[
            ("addsTwo", &format!("{tool} test --tests 'LibTest.addsTwo'")),
            (
                "adds two numbers",
                &format!("{tool} test --tests 'LibTest.adds two numbers'"),
            ),
            (
                "innerCase",
                &format!("{tool} test --tests 'LibTest$Inner.innerCase'"),
            ),
        ],
    );
}

#[test]
fn a_targeted_gradle_command_takes_the_windows_wrapper_only_on_windows() {
    let repo = scanned_repo(&[
        ("gradlew.bat", "@echo off\n"),
        ("app/build.gradle", "plugins { id 'java' }\n"),
        (
            "app/src/main/java/x/Foo.java",
            "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n",
        ),
        (
            "app/src/test/java/x/FooTest.java",
            "package x;\n\nimport org.junit.Test;\n\npublic class FooTest {\n    @Test\n    public void checksTarget() {\n        Foo.target();\n    }\n}\n",
        ),
    ]);
    let expected = if cfg!(windows) {
        "cd app && ..\\gradlew.bat test --tests 'FooTest.checksTarget'"
    } else {
        "cd app && gradle test --tests 'FooTest.checksTarget'"
    };

    let target = report(&repo, "target", "app/src/main/java/x/Foo.java");

    assert_commands(&target, &[("checksTarget", expected)]);
}

const JAVA_LIB: &str = "package x;\n\npublic class Foo {\n    public static int target() {\n        return 1;\n    }\n}\n";

fn java_test(class: &str) -> String {
    format!(
        "package x;\n\nimport org.junit.Test;\n\npublic class {class} {{\n    @Test\n    public void checksTarget() {{\n        Foo.target();\n    }}\n}}\n"
    )
}

const WRAPPER: &str = "./gradlew";

#[test]
fn java_kotlin_and_scala_tests_in_gradle_subprojects_run_only_their_project_task() {
    let repo = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        (
            "settings.gradle.kts",
            "rootProject.name = \"demo-root\"\n\ninclude(\":lib\")\n\ninclude(\":kotlin-tests\")\n\ninclude(\":kotlin-tests:codegen-only\")\n\ninclude(\":scala\")\n",
        ),
        ("build.gradle.kts", "plugins { java }\n"),
        ("lib/build.gradle.kts", "plugins { java }\n"),
        ("lib/src/main/java/x/Foo.java", JAVA_LIB),
        ("lib/src/test/java/x/FooTest.java", &java_test("FooTest")),
        (
            "kotlin-tests/build.gradle.kts",
            "plugins { kotlin(\"jvm\") }\n",
        ),
        (
            "kotlin-tests/src/main/kotlin/demo/Lib.kt",
            &KOTLIN_LIB.replace("target", "kotlinTarget"),
        ),
        (
            "kotlin-tests/src/test/kotlin/demo/LibTest.kt",
            &KOTLIN_TEST.replace("target", "kotlinTarget"),
        ),
        (
            "kotlin-tests/codegen-only/build.gradle.kts",
            "plugins { kotlin(\"jvm\") }\n",
        ),
        (
            "kotlin-tests/codegen-only/src/test/kotlin/demo/CodegenTest.kt",
            "package demo\n\nimport org.junit.jupiter.api.Test\n\nclass CodegenTest {\n    @Test fun usesTarget() { kotlinTarget() }\n}\n",
        ),
        ("scala/build.gradle", "plugins { id 'scala' }\n"),
        ("scala/src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("scala/src/test/scala/demo/LibSuite.scala", SCALA_TEST),
    ]);
    let tool = WRAPPER;

    assert_commands(
        &report(&repo, "target", "lib/src/main/java/x/Foo.java"),
        &[(
            "checksTarget",
            &format!("{tool} :lib:test --tests 'FooTest.checksTarget'"),
        )],
    );
    assert_commands(
        &report(
            &repo,
            "kotlinTarget",
            "kotlin-tests/src/main/kotlin/demo/Lib.kt",
        ),
        &[
            (
                "usesTarget",
                &format!("{tool} :kotlin-tests:codegen-only:test --tests 'CodegenTest.usesTarget'"),
            ),
            (
                "addsTwo",
                &format!("{tool} :kotlin-tests:test --tests 'LibTest.addsTwo'"),
            ),
            (
                "adds two numbers",
                &format!("{tool} :kotlin-tests:test --tests 'LibTest.adds two numbers'"),
            ),
            (
                "innerCase",
                &format!("{tool} :kotlin-tests:test --tests 'LibTest$Inner.innerCase'"),
            ),
        ],
    );
    assert_commands(
        &report(&repo, "target", "scala/src/main/scala/demo/Lib.scala"),
        &[
            (
                "addsTwo",
                &format!("{tool} :scala:test --tests 'LibJUnitTest.addsTwo'"),
            ),
            ("adds two", "-"),
        ],
    );
}

#[test]
fn a_groovy_settings_file_moves_a_project_and_names_the_root_project_task() {
    let repo = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        (
            "settings.gradle",
            "rootProject.name = 'demo'\ninclude 'app',\n    ':core'\nproject(':core').projectDir = new File(settingsDir, 'libs/core')\n",
        ),
        ("build.gradle", "plugins { id 'java' }\n"),
        ("src/main/java/x/Foo.java", JAVA_LIB),
        ("src/test/java/x/RootTest.java", &java_test("RootTest")),
        ("libs/core/build.gradle", "plugins { id 'java' }\n"),
        (
            "libs/core/src/test/java/x/CoreTest.java",
            &java_test("CoreTest"),
        ),
    ]);
    let tool = WRAPPER;

    assert_commands(
        &report(&repo, "target", "src/main/java/x/Foo.java"),
        &[
            (
                "checksTarget",
                &format!("{tool} :core:test --tests 'CoreTest.checksTarget'"),
            ),
            (
                "checksTarget",
                &format!("{tool} :test --tests 'RootTest.checksTarget'"),
            ),
        ],
    );
}

#[test]
fn a_gradle_project_that_a_condition_includes_has_no_command_and_a_single_project_keeps_its_form() {
    let conditional = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        (
            "settings.gradle.kts",
            "if (JavaVersion.current().isJava11Compatible) {\n    include(\":records\")\n}\n",
        ),
        ("records/build.gradle.kts", "plugins { java }\n"),
        ("records/src/main/java/x/Foo.java", JAVA_LIB),
        (
            "records/src/test/java/x/FooTest.java",
            &java_test("FooTest"),
        ),
    ]);
    let single = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        ("settings.gradle", "rootProject.name = 'solo'\n"),
        ("build.gradle", "plugins { id 'java' }\n"),
        ("src/main/java/x/Foo.java", JAVA_LIB),
        ("src/test/java/x/FooTest.java", &java_test("FooTest")),
    ]);

    let braceless = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        (
            "settings.gradle.kts",
            "if (System.getenv(\"CI\") != null)\n    include(\":extra\")\n",
        ),
        ("extra/build.gradle.kts", "plugins { java }\n"),
        ("extra/src/main/java/x/Foo.java", JAVA_LIB),
        ("extra/src/test/java/x/FooTest.java", &java_test("FooTest")),
    ]);

    assert_commands(
        &report(&conditional, "target", "records/src/main/java/x/Foo.java"),
        &[("checksTarget", "-")],
    );
    assert_commands(
        &report(&braceless, "target", "extra/src/main/java/x/Foo.java"),
        &[("checksTarget", "-")],
    );
    assert_commands(
        &report(&single, "target", "src/main/java/x/Foo.java"),
        &[(
            "checksTarget",
            &format!("{WRAPPER} test --tests 'FooTest.checksTarget'"),
        )],
    );
}

#[test]
fn a_buildsrc_test_beside_a_multi_project_settings_file_runs_in_its_own_build() {
    let repo = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        ("settings.gradle.kts", "include(\":a\")\n"),
        ("build.gradle.kts", "plugins { java }\n"),
        ("buildSrc/build.gradle.kts", "plugins { java }\n"),
        ("buildSrc/src/main/java/x/Foo.java", JAVA_LIB),
        (
            "buildSrc/src/test/java/x/BuildSrcTest.java",
            &java_test("BuildSrcTest"),
        ),
        ("a/build.gradle.kts", "plugins { java }\n"),
        (
            "a/src/main/java/y/Bar.java",
            &JAVA_LIB
                .replace("package x;", "package y;")
                .replace("Foo", "Bar"),
        ),
        (
            "a/src/test/java/y/BarTest.java",
            &java_test("BarTest")
                .replace("package x;", "package y;")
                .replace("Foo.", "Bar."),
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "buildSrc/src/main/java/x/Foo.java"),
        &[(
            "checksTarget",
            "cd buildSrc && ../gradlew test --tests 'BuildSrcTest.checksTarget'",
        )],
    );
    assert_commands(
        &report(&repo, "target", "a/src/main/java/y/Bar.java"),
        &[(
            "checksTarget",
            &format!("{WRAPPER} :a:test --tests 'BarTest.checksTarget'"),
        )],
    );
}

#[test]
fn a_project_that_a_sibling_settings_file_includes_flat_runs_from_that_folder() {
    let repo = scanned_repo(&[
        ("gradlew", "#!/bin/sh\n"),
        ("build/settings.gradle", "includeFlat 'x', 'y'\n"),
        (
            "x/build.gradle.kts",
            "plugins { java }\ndependencies { implementation(project(\":y\")) }\n",
        ),
        ("x/src/main/java/x/Foo.java", JAVA_LIB),
        ("x/src/test/java/x/FooTest.java", &java_test("FooTest")),
        ("y/build.gradle.kts", "plugins { java }\n"),
        ("solo/build.gradle.kts", "plugins { java }\n"),
        (
            "solo/src/main/java/s/Solo.java",
            &JAVA_LIB
                .replace("package x;", "package s;")
                .replace("Foo", "Solo"),
        ),
        (
            "solo/src/test/java/s/SoloTest.java",
            &java_test("SoloTest")
                .replace("package x;", "package s;")
                .replace("Foo.", "Solo."),
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "x/src/main/java/x/Foo.java"),
        &[(
            "checksTarget",
            "cd build && ../gradlew :x:test --tests 'FooTest.checksTarget'",
        )],
    );
    assert_commands(
        &report(&repo, "target", "solo/src/main/java/s/Solo.java"),
        &[(
            "checksTarget",
            "cd solo && ../gradlew test --tests 'SoloTest.checksTarget'",
        )],
    );
}

const SCALA_LIB: &str = "package demo\n\nobject Lib {\n  def target(): Int = 1\n}\n";
const SCALA_TEST: &str = "package demo\n\nimport org.junit.Test\nimport org.scalatest.funsuite.AnyFunSuite\n\nclass LibJUnitTest {\n  @Test def addsTwo(): Unit = { Lib.target() }\n}\n\nclass LibSuite extends AnyFunSuite {\n  test(\"adds two\") { Lib.target() }\n}\n";

#[test]
fn scala_tests_run_through_sbt_with_the_scalatest_name_filter() {
    let repo = scanned_repo(&[
        (
            "build.sbt",
            "libraryDependencies += \"org.scalatest\" %% \"scalatest\" % \"3.2.18\" % Test\n",
        ),
        ("src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("src/test/scala/demo/LibSuite.scala", SCALA_TEST),
    ]);

    let target = report(&repo, "target", "src/main/scala/demo/Lib.scala");

    assert_commands(
        &target,
        &[
            ("addsTwo", "sbt 'testOnly *LibJUnitTest'"),
            ("adds two", "sbt 'testOnly *LibSuite -- -z \"adds two\"'"),
        ],
    );
}

#[test]
fn a_scalatest_label_inside_a_describe_block_runs_through_its_spec_class() {
    let repo = scanned_repo(&[
        (
            "build.sbt",
            "libraryDependencies += \"org.scalatest\" %% \"scalatest\" % \"3.2.18\" % Test\n",
        ),
        ("src/main/scala/demo/Lib.scala", SCALA_LIB),
        (
            "src/test/scala/demo/LibSpec.scala",
            "package demo\n\nimport org.scalatest.funspec.AnyFunSpec\n\nclass LibSpec extends AnyFunSpec {\n  describe(\"calc\") {\n    it(\"adds two\") {\n      Lib.target()\n    }\n  }\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/main/scala/demo/Lib.scala"),
        &[("adds two", "sbt 'testOnly *LibSpec -- -z \"adds two\"'")],
    );
}

const MAVEN_REACTOR: &str = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>x</groupId>\n  <artifactId>parent</artifactId>\n  <version>1.0-SNAPSHOT</version>\n  <packaging>pom</packaging>\n  <modules>\n    <module>lib</module>\n    <module>kotlin-tests</module>\n    <module>scala</module>\n  </modules>\n  <profiles>\n    <profile>\n      <id>bench</id>\n      <modules>\n        <module>bench</module>\n      </modules>\n    </profile>\n  </profiles>\n</project>\n";
const MAVEN_MODULE: &str = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <parent>\n    <groupId>x</groupId>\n    <artifactId>parent</artifactId>\n    <version>1.0-SNAPSHOT</version>\n  </parent>\n  <artifactId>module</artifactId>\n</project>\n";
const MAVEN_FLAGS: &str = "-Dsurefire.failIfNoSpecifiedTests=false";

#[test]
fn java_kotlin_and_scala_tests_in_maven_modules_run_from_the_reactor_root() {
    let repo = scanned_repo(&[
        ("pom.xml", MAVEN_REACTOR),
        ("lib/pom.xml", MAVEN_MODULE),
        ("lib/src/main/java/x/Foo.java", JAVA_LIB),
        ("lib/src/test/java/x/FooTest.java", &java_test("FooTest")),
        ("bench/pom.xml", MAVEN_MODULE),
        (
            "bench/src/test/java/x/BenchTest.java",
            &java_test("BenchTest"),
        ),
        (
            "samples/demo/pom.xml",
            "<project>\n  <parent>\n    <groupId>x</groupId>\n    <artifactId>parent</artifactId>\n    <relativePath>../../pom.xml</relativePath>\n  </parent>\n</project>\n",
        ),
        (
            "samples/demo/src/test/java/x/DemoTest.java",
            &java_test("DemoTest"),
        ),
        (
            "tools/pom.xml",
            "<project>\n  <artifactId>tools</artifactId>\n</project>\n",
        ),
        (
            "tools/src/test/java/x/ToolTest.java",
            &java_test("ToolTest"),
        ),
        ("kotlin-tests/pom.xml", MAVEN_MODULE),
        (
            "kotlin-tests/src/main/kotlin/demo/Lib.kt",
            &KOTLIN_LIB.replace("target", "kotlinTarget"),
        ),
        (
            "kotlin-tests/src/test/kotlin/demo/LibTest.kt",
            &KOTLIN_TEST.replace("target", "kotlinTarget"),
        ),
        ("scala/pom.xml", MAVEN_MODULE),
        ("scala/src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("scala/src/test/scala/demo/LibSuite.scala", SCALA_TEST),
    ]);

    assert_commands(
        &report(&repo, "target", "lib/src/main/java/x/Foo.java"),
        &[
            ("checksTarget", "-"),
            (
                "checksTarget",
                &format!("mvn package -pl lib -am -Dtest=x.FooTest#checksTarget {MAVEN_FLAGS}"),
            ),
            ("checksTarget", "-"),
            (
                "checksTarget",
                "cd tools && mvn test -Dtest=x.ToolTest#checksTarget",
            ),
        ],
    );
    assert_commands(
        &report(
            &repo,
            "kotlinTarget",
            "kotlin-tests/src/main/kotlin/demo/Lib.kt",
        ),
        &[
            (
                "addsTwo",
                &format!(
                    "mvn package -pl kotlin-tests -am -Dtest=demo.LibTest#addsTwo {MAVEN_FLAGS}"
                ),
            ),
            (
                "adds two numbers",
                &format!(
                    "mvn package -pl kotlin-tests -am -Dtest=demo.LibTest#'adds two numbers' {MAVEN_FLAGS}"
                ),
            ),
            (
                "innerCase",
                &format!(
                    "mvn package -pl kotlin-tests -am -Dtest='demo.LibTest$Inner'#innerCase {MAVEN_FLAGS}"
                ),
            ),
        ],
    );
    assert_commands(
        &report(&repo, "target", "scala/src/main/scala/demo/Lib.scala"),
        &[
            (
                "addsTwo",
                &format!(
                    "mvn package -pl scala -am -Dtest=demo.LibJUnitTest#addsTwo {MAVEN_FLAGS}"
                ),
            ),
            ("adds two", "-"),
        ],
    );
}

const SPRING_CHILD: &str = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <parent>\n    <groupId>org.springframework.boot</groupId>\n    <artifactId>spring-boot-starter-parent</artifactId>\n    <version>3.3.0</version>\n  </parent>\n  <groupId>x</groupId>\n  <artifactId>tools</artifactId>\n</project>\n";
const QUOTED_BRACKET_POM: &str = "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>x</groupId>\n  <artifactId>solo</artifactId>\n  <version>1</version>\n  <build>\n    <plugins>\n      <plugin>\n        <artifactId>maven-antrun-plugin</artifactId>\n        <configuration>\n          <target>\n            <echo message=\"generated -> target\"/>\n          </target>\n        </configuration>\n      </plugin>\n    </plugins>\n  </build>\n</project>\n";

#[test]
fn maven_tests_of_nested_aggregators_external_parents_and_quoted_brackets_run_as_maven_builds_them()
{
    let repo = scanned_repo(&[
        (
            "pom.xml",
            &MAVEN_REACTOR.replace(
                "<module>lib</module>",
                "<module>lib</module>\n    <module>libs</module>",
            ),
        ),
        ("lib/pom.xml", MAVEN_MODULE),
        ("lib/src/main/java/x/Foo.java", JAVA_LIB),
        (
            "lib/src/test/java/y/FooTest.java",
            &java_test("FooTest").replace("package x;", "package y;\n\nimport x.Foo;"),
        ),
        (
            "libs/pom.xml",
            "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <parent>\n    <groupId>x</groupId>\n    <artifactId>parent</artifactId>\n    <version>1.0-SNAPSHOT</version>\n  </parent>\n  <artifactId>libs</artifactId>\n  <packaging>pom</packaging>\n  <modules>\n    <module>json</module>\n  </modules>\n</project>\n",
        ),
        ("libs/src/test/java/x/LibsTest.java", &java_test("LibsTest")),
        ("libs/json/pom.xml", MAVEN_MODULE),
        (
            "libs/json/src/test/java/x/FooTest.java",
            &java_test("FooTest"),
        ),
        ("tools/pom.xml", SPRING_CHILD),
        (
            "tools/src/test/java/x/ToolTest.java",
            &java_test("ToolTest"),
        ),
        ("solo/pom.xml", QUOTED_BRACKET_POM),
        ("solo/src/test/java/x/SoloTest.java", &java_test("SoloTest")),
    ]);

    assert_commands(
        &report(&repo, "target", "lib/src/main/java/x/Foo.java"),
        &[
            (
                "checksTarget",
                &format!("mvn package -pl lib -am -Dtest=y.FooTest#checksTarget {MAVEN_FLAGS}"),
            ),
            (
                "checksTarget",
                &format!(
                    "mvn package -pl libs/json -am -Dtest=x.FooTest#checksTarget {MAVEN_FLAGS}"
                ),
            ),
            ("checksTarget", "-"),
            (
                "checksTarget",
                "cd solo && mvn test -Dtest=x.SoloTest#checksTarget",
            ),
            (
                "checksTarget",
                "cd tools && mvn test -Dtest=x.ToolTest#checksTarget",
            ),
        ],
    );
}

#[test]
fn a_scala_test_of_an_sbt_build_nested_in_a_subproject_folder_runs_in_that_build() {
    let repo = scanned_repo(&[
        (
            "build.sbt",
            "lazy val root = (project in file(\".\")).aggregate(core)\n\nlazy val core = project\n  .settings(libraryDependencies += \"org.scalatest\" %% \"scalatest\" % \"3.2.18\" % Test)\n",
        ),
        ("project/build.properties", "sbt.version=1.10.7\n"),
        ("core/src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("core/src/test/scala/demo/LibSuite.scala", SCALA_TEST),
        (
            "core/examples/demo/build.sbt",
            "libraryDependencies += \"org.scalatest\" %% \"scalatest\" % \"3.2.18\" % Test\n",
        ),
        (
            "core/examples/demo/project/build.properties",
            "sbt.version=1.10.7\n",
        ),
        (
            "core/examples/demo/src/main/scala/ex/Ex.scala",
            "package ex\n\nobject Ex {\n  def exampleTarget(): Int = 1\n}\n",
        ),
        (
            "core/examples/demo/src/test/scala/ex/ExSuite.scala",
            "package ex\n\nimport org.scalatest.funsuite.AnyFunSuite\n\nclass ExSuite extends AnyFunSuite {\n  test(\"example works\") { Ex.exampleTarget() }\n}\n",
        ),
    ]);

    assert_commands(
        &report(
            &repo,
            "exampleTarget",
            "core/examples/demo/src/main/scala/ex/Ex.scala",
        ),
        &[(
            "example works",
            "cd core/examples/demo && sbt 'testOnly *ExSuite -- -z \"example works\"'",
        )],
    );
    assert_commands(
        &report(&repo, "target", "core/src/main/scala/demo/Lib.scala"),
        &[
            ("addsTwo", "sbt 'core/testOnly *LibJUnitTest'"),
            (
                "adds two",
                "sbt 'core/testOnly *LibSuite -- -z \"adds two\"'",
            ),
        ],
    );
}

#[test]
fn scala_tests_of_an_sbt_subproject_run_from_the_build_root_by_project_id() {
    let repo = scanned_repo(&[
        (
            "build.sbt",
            "lazy val root = (project in file(\".\")).aggregate(core)\n\nlazy val core = project\n  .settings(libraryDependencies += \"org.scalatest\" %% \"scalatest\" % \"3.2.18\" % Test)\n",
        ),
        ("core/build.sbt", "name := \"core\"\n"),
        ("core/src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("core/src/test/scala/demo/LibSuite.scala", SCALA_TEST),
    ]);

    assert_commands(
        &report(&repo, "target", "core/src/main/scala/demo/Lib.scala"),
        &[
            ("addsTwo", "sbt 'core/testOnly *LibJUnitTest'"),
            (
                "adds two",
                "sbt 'core/testOnly *LibSuite -- -z \"adds two\"'",
            ),
        ],
    );
}

#[test]
fn scala_junit_methods_run_through_maven_like_java_but_a_scalatest_label_does_not() {
    let repo = scanned_repo(&[
        ("pom.xml", "<project></project>\n"),
        ("src/main/scala/demo/Lib.scala", SCALA_LIB),
        ("src/test/scala/demo/LibSuite.scala", SCALA_TEST),
    ]);

    let target = report(&repo, "target", "src/main/scala/demo/Lib.scala");

    assert_commands(
        &target,
        &[
            ("addsTwo", "mvn test -Dtest=demo.LibJUnitTest#addsTwo"),
            ("adds two", "-"),
        ],
    );
}

#[test]
fn fsharp_and_vbnet_tests_run_through_dotnet_test_with_their_module_or_class() {
    let fsharp = scanned_repo(&[(
        "tests/Tests.fs",
        "module Demo.Tests\n\nopen Xunit\n\nlet target () = 1\n\n[<Fact>]\nlet ``adds two`` () = Assert.Equal(1, target ())\n\ntype CalcTests() =\n    [<Fact>]\n    member _.Adds() = Assert.Equal(1, target ())\n",
    )]);
    let vbnet = scanned_repo(&[(
        "tests/CalcTests.vb",
        "Imports Xunit\n\nPublic Class CalcTests\n    <Fact>\n    Public Sub Adds()\n        Assert.Equal(1, Target())\n    End Sub\n\n    Function Target() As Integer\n        Return 1\n    End Function\nEnd Class\n",
    )]);

    let fsharp_report = report(&fsharp, "target", "tests/Tests.fs");
    let vbnet_report = report(&vbnet, "Target", "tests/CalcTests.vb");

    assert_commands(
        &fsharp_report,
        &[
            (
                "``adds two``",
                "dotnet test --filter \"FullyQualifiedName~Demo.Tests.adds two\"",
            ),
            (
                "Adds",
                "dotnet test --filter \"FullyQualifiedName~CalcTests.Adds\"",
            ),
        ],
    );
    assert_commands(
        &vbnet_report,
        &[(
            "Adds",
            "dotnet test --filter \"FullyQualifiedName~CalcTests.Adds\"",
        )],
    );
}

const RUBY_LIB: &str = "def target\n  1\nend\n";
const RUBY_SPEC: &str = "require \"calc\"\n\nRSpec.describe \"calc\" do\n  it \"adds two\" do\n    expect(target).to eq(1)\n  end\nend\n";
const RUBY_MINITEST: &str = "require \"minitest/autorun\"\n\nclass CalcTest < Minitest::Test\n  def test_adds\n    assert_equal 1, target\n  end\nend\n";

#[test]
fn an_rspec_example_runs_by_its_line_when_the_project_configures_rspec() {
    let configured = scanned_repo(&[
        (
            "Gemfile",
            "source \"https://rubygems.org\"\ngem \"rspec\"\n",
        ),
        ("lib/calc.rb", RUBY_LIB),
        ("spec/calc_spec.rb", RUBY_SPEC),
    ]);
    let unconfigured = scanned_repo(&[
        ("Gemfile", "source \"https://rubygems.org\"\ngem \"rake\"\n"),
        ("lib/calc.rb", RUBY_LIB),
        ("spec/calc_spec.rb", RUBY_SPEC),
    ]);
    let line = line_of(&configured, "spec/calc_spec.rb", "it \"adds two\"");

    let configured_report = report(&configured, "target", "lib/calc.rb");
    let unconfigured_report = report(&unconfigured, "target", "lib/calc.rb");

    assert_commands(
        &configured_report,
        &[(
            "adds two",
            &format!("bundle exec rspec spec/calc_spec.rb:{line}"),
        )],
    );
    assert_commands(&unconfigured_report, &[("adds two", "-")]);
}

#[test]
fn a_minitest_test_runs_through_rails_or_a_rake_test_task() {
    let rails = scanned_repo(&[
        ("Gemfile", "gem \"rails\"\n"),
        ("bin/rails", "#!/usr/bin/env ruby\n"),
        ("lib/calc.rb", RUBY_LIB),
        ("test/calc_test.rb", RUBY_MINITEST),
    ]);
    let rake = scanned_repo(&[
        (
            "Rakefile",
            "require \"rake/testtask\"\nRake::TestTask.new(:test)\n",
        ),
        ("lib/calc.rb", RUBY_LIB),
        ("test/calc_test.rb", RUBY_MINITEST),
    ]);
    let line = line_of(&rails, "test/calc_test.rb", "def test_adds");

    let rails_report = report(&rails, "target", "lib/calc.rb");
    let rake_report = report(&rake, "target", "lib/calc.rb");

    assert_commands(
        &rails_report,
        &[(
            "test_adds",
            &format!("bin/rails test test/calc_test.rb:{line}"),
        )],
    );
    assert_commands(
        &rake_report,
        &[(
            "test_adds",
            "rake test TEST=test/calc_test.rb TESTOPTS='--name=test_adds'",
        )],
    );
}

#[test]
fn swift_tests_run_through_swift_test_with_a_filter() {
    let repo = scanned_repo(&[
        (
            "Package.swift",
            "// swift-tools-version:5.9\nimport PackageDescription\nlet package = Package(name: \"Calc\")\n",
        ),
        (
            "Sources/Calc/Calc.swift",
            "public func target() -> Int { 1 }\n",
        ),
        (
            "Tests/CalcTests/CalcTests.swift",
            "import XCTest\nimport Testing\n@testable import Calc\n\nfinal class CalcTests: XCTestCase {\n    func testAdds() { XCTAssertEqual(target(), 1) }\n}\n\n@Test func addsTwo() { #expect(target() == 1) }\n",
        ),
    ]);

    let target = report(&repo, "target", "Sources/Calc/Calc.swift");

    assert_commands(
        &target,
        &[
            ("testAdds", "swift test --filter 'CalcTests/testAdds\\b'"),
            ("addsTwo", "swift test --filter '[./]addsTwo\\b'"),
        ],
    );
}

const DART_LIB: &str = "int target() => 1;\n";
const DART_TEST: &str = "import 'package:test/test.dart';\nimport 'package:calc/calc.dart';\n\nvoid main() {\n  group('calc', () {\n    test('adds two', () {\n      expect(target(), 1);\n    });\n  });\n}\n";

#[test]
fn dart_tests_run_through_dart_or_flutter_test_by_plain_name() {
    let dart = scanned_repo(&[
        (
            "pubspec.yaml",
            "name: calc\ndev_dependencies:\n  test: ^1.24.0\n",
        ),
        ("lib/calc.dart", DART_LIB),
        ("test/calc_test.dart", DART_TEST),
    ]);
    let flutter = scanned_repo(&[
        (
            "pubspec.yaml",
            "name: calc\ndev_dependencies:\n  flutter_test:\n    sdk: flutter\n",
        ),
        ("lib/calc.dart", DART_LIB),
        ("test/calc_test.dart", DART_TEST),
    ]);
    let none = scanned_repo(&[
        ("pubspec.yaml", "name: calc\n"),
        ("lib/calc.dart", DART_LIB),
        ("test/calc_test.dart", DART_TEST),
    ]);

    assert_commands(
        &report(&dart, "target", "lib/calc.dart"),
        &[(
            "adds two",
            "dart test test/calc_test.dart --plain-name 'adds two'",
        )],
    );
    assert_commands(
        &report(&flutter, "target", "lib/calc.dart"),
        &[(
            "adds two",
            "flutter test test/calc_test.dart --plain-name 'adds two'",
        )],
    );
    assert_commands(
        &report(&none, "target", "lib/calc.dart"),
        &[("adds two", "-")],
    );
}

#[test]
fn a_googletest_case_runs_through_ctest_in_the_one_configured_build_folder() {
    let files = [
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(calc)\nenable_testing()\ninclude(GoogleTest)\nadd_executable(calc_test tests/calc_test.cpp)\ngtest_discover_tests(calc_test)\n",
        ),
        ("src/calc.h", "inline int target() { return 1; }\n"),
        (
            "tests/calc_test.cpp",
            "#include <gtest/gtest.h>\n#include \"calc.h\"\n\nTEST(CalcTest, Adds) { EXPECT_EQ(target(), 1); }\n",
        ),
    ];
    let built: Vec<(&str, &str)> = files
        .iter()
        .copied()
        .chain([("build/CTestTestfile.cmake", "subdirs()\n")])
        .collect();
    let configured = scanned_repo(&built);
    let unbuilt = scanned_repo(&files);

    assert_commands(
        &report(&configured, "target", "src/calc.h"),
        &[(
            "CalcTest.Adds",
            "ctest --test-dir build -R '^CalcTest\\.Adds$'",
        )],
    );
    assert_commands(
        &report(&unbuilt, "target", "src/calc.h"),
        &[("CalcTest.Adds", "-")],
    );
}

#[test]
fn a_phpunit_test_runs_with_a_class_and_method_filter() {
    let repo = scanned_repo(&[
        (
            "composer.json",
            "{\"require-dev\": {\"phpunit/phpunit\": \"^10\"}}\n",
        ),
        ("src/calc.php", "<?php\nfunction target() { return 1; }\n"),
        (
            "tests/CalcTest.php",
            "<?php\nuse PHPUnit\\Framework\\TestCase;\n\nfinal class CalcTest extends TestCase {\n    public function testAdds(): void { $this->assertSame(1, target()); }\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/calc.php"),
        &[(
            "testAdds",
            "vendor/bin/phpunit --filter 'CalcTest::testAdds\\b' tests/CalcTest.php",
        )],
    );
}

#[test]
fn an_exunit_test_runs_through_mix_by_its_line() {
    let repo = scanned_repo(&[
        (
            "mix.exs",
            "defmodule Calc.MixProject do\n  use Mix.Project\nend\n",
        ),
        (
            "lib/calc.ex",
            "defmodule Calc do\n  def target, do: 1\nend\n",
        ),
        (
            "test/calc_test.exs",
            "defmodule CalcTest do\n  use ExUnit.Case\n\n  test \"adds two\" do\n    assert Calc.target() == 1\n  end\nend\n",
        ),
    ]);
    let line = line_of(&repo, "test/calc_test.exs", "test \"adds two\"");

    assert_commands(
        &report(&repo, "target", "lib/calc.ex"),
        &[("adds two", &format!("mix test test/calc_test.exs:{line}"))],
    );
}

#[test]
fn erlang_tests_run_through_rebar3_eunit_or_common_test() {
    let repo = scanned_repo(&[
        ("rebar.config", "{erl_opts, [debug_info]}.\n"),
        (
            "src/calc.erl",
            "-module(calc).\n-export([target/0]).\n\ntarget() -> 1.\n",
        ),
        (
            "test/calc_tests.erl",
            "-module(calc_tests).\n-include_lib(\"eunit/include/eunit.hrl\").\n\nadds_test() -> ?assertEqual(1, calc:target()).\n",
        ),
        (
            "test/calc_SUITE.erl",
            "-module(calc_SUITE).\n-export([all/0, adds/1]).\n\nall() -> [adds].\n\nadds(_Config) -> 1 = calc:target().\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/calc.erl"),
        &[
            ("adds", "rebar3 ct --suite=test/calc_SUITE --case=adds"),
            ("adds_test", "rebar3 eunit --test='calc_tests:adds_test'"),
        ],
    );
}

#[test]
fn a_busted_test_runs_with_a_lua_pattern_filter() {
    let repo = scanned_repo(&[
        (".busted", "return { default = { ROOT = { \"spec\" } } }\n"),
        ("src/calc.lua", "function target()\n  return 1\nend\n"),
        (
            "spec/calc_spec.lua",
            "require(\"calc\")\n\ndescribe(\"calc\", function()\n  it(\"adds two-fold\", function()\n    assert.are.equal(1, target())\n  end)\nend)\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/calc.lua"),
        &[(
            "adds two-fold",
            "busted spec/calc_spec.lua --filter='adds two%-fold$'",
        )],
    );
}

#[test]
fn a_testthat_test_runs_through_test_file_with_its_description() {
    let repo = scanned_repo(&[
        (
            "DESCRIPTION",
            "Package: calc\nVersion: 0.1.0\nSuggests: testthat (>= 3.2.0)\n",
        ),
        ("R/calc.R", "target <- function() 1\n"),
        (
            "tests/testthat/test-calc.R",
            "test_that(\"adds two\", {\n  expect_equal(target(), 1)\n})\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "R/calc.R"),
        &[(
            "adds two",
            "Rscript -e 'testthat::test_file(\"tests/testthat/test-calc.R\", desc = \"adds two\", package = \"calc\", load_package = \"source\")'",
        )],
    );
}

#[test]
fn a_pester_test_runs_with_a_full_name_filter() {
    let repo = scanned_repo(&[
        ("src/Calc.ps1", "function Get-Target { 1 }\n"),
        (
            "tests/Calc.Tests.ps1",
            "Describe \"calc\" {\n    It \"adds [two]\" {\n        Get-Target | Should -Be 1\n    }\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "Get-Target", "src/Calc.ps1"),
        &[(
            "adds [two]",
            "pwsh -NoProfile -Command \"Invoke-Pester -Path 'tests/Calc.Tests.ps1' -FullNameFilter '*.adds \\`[two\\`]'\"",
        )],
    );
}

#[test]
fn a_bats_test_runs_with_an_anchored_name_filter() {
    let repo = scanned_repo(&[
        ("lib/calc.sh", "target() {\n  echo 1\n}\n"),
        (
            "test/calc.bats",
            "load ../lib/calc.sh\n\n@test \"adds two (fast)\" {\n  target\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "lib/calc.sh"),
        &[(
            "adds two (fast)",
            "bats test/calc.bats --filter '^adds two \\(fast\\)$'",
        )],
    );
}

#[test]
fn a_gut_test_runs_through_the_gut_command_line_of_its_godot_project() {
    let repo = scanned_repo(&[
        ("game/project.godot", "config_version=5\n"),
        ("game/addons/gut/gut_cmdln.gd", "extends SceneTree\n"),
        (
            "game/test/test_calc.gd",
            "extends GutTest\n\nfunc target():\n\treturn 1\n\nfunc test_adds():\n\tassert_eq(target(), 1)\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "game/test/test_calc.gd"),
        &[(
            "test_adds",
            "cd game && godot --headless -s addons/gut/gut_cmdln.gd '-gtest=res://test/test_calc.gd' '-gunit_test_name=test_adds' -gexit",
        )],
    );
}

const ZIG_CALC: &str = "const std = @import(\"std\");\n\nfn helper() i32 {\n    return 1;\n}\n\nfn twice() i32 {\n    return helper() * 2;\n}\n\ntest \"adds two\" {\n    try std.testing.expectEqual(@as(i32, 1), helper());\n}\n\ntest twice {\n    try std.testing.expectEqual(helper() * 2, twice());\n}\n\ntest {\n    _ = helper();\n}\n";

#[test]
fn a_zig_test_runs_through_zig_test_with_the_name_the_test_runner_gives_it() {
    let repo = scanned_repo(&[
        (
            "build.zig",
            "const std = @import(\"std\");\n\npub fn build(b: *std.Build) void {\n    _ = b;\n}\n",
        ),
        ("src/calc.zig", ZIG_CALC),
    ]);

    assert_commands(
        &report(&repo, "helper", "src/calc.zig"),
        &[
            (
                "adds two",
                "zig test src/calc.zig --test-filter 'test.adds two'",
            ),
            (
                "twice",
                "zig test src/calc.zig --test-filter 'decltest.twice'",
            ),
            ("test", "-"),
        ],
    );
}

#[test]
fn a_zig_build_with_a_test_filter_option_runs_the_test_step() {
    let repo = scanned_repo(&[
        (
            "lib/build.zig",
            "const std = @import(\"std\");\n\npub fn build(b: *std.Build) void {\n    const filters = b.option([]const []const u8, \"test-filter\", \"Skip tests that do not match\") orelse &.{};\n    const tests = b.addTest(.{ .root_source_file = b.path(\"src/calc.zig\"), .filters = filters });\n    const test_step = b.step(\"test\", \"Run unit tests\");\n    test_step.dependOn(&b.addRunArtifact(tests).step);\n}\n",
        ),
        ("lib/src/calc.zig", ZIG_CALC),
    ]);

    assert_commands(
        &report(&repo, "helper", "lib/src/calc.zig"),
        &[
            (
                "adds two",
                "cd lib && zig build test -Dtest-filter='test.adds two'",
            ),
            (
                "twice",
                "cd lib && zig build test -Dtest-filter='decltest.twice'",
            ),
            ("test", "-"),
        ],
    );
}

#[test]
fn a_qt_quick_test_runs_through_qmltestrunner_with_its_test_case_name() {
    let repo = scanned_repo(&[
        (
            "tests/tst_calc.qml",
            "import QtQuick\nimport QtTest\n\nTestCase {\n    name: \"CalcTests\" // the reported name\n    function target() { return 1 }\n    function test_adds() {\n        compare(target(), 1)\n    }\n    TestCase {\n        function test_unnamed() {\n            compare(target(), 1)\n        }\n    }\n}\n",
        ),
        (
            "tests/calc_checks.qml",
            "import QtQuick\nimport QtTest\n\nTestCase {\n    name: \"Checks\"\n    function target() { return 1 }\n    function test_adds() {\n        compare(target(), 1)\n    }\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "tests/tst_calc.qml"),
        &[
            (
                "test_adds",
                "qmltestrunner -platform offscreen -input tests/tst_calc.qml 'CalcTests::test_adds'",
            ),
            ("test_unnamed", "-"),
        ],
    );
    assert_commands(
        &report(&repo, "target", "tests/calc_checks.qml"),
        &[("test_adds", "-")],
    );
}

const C_HEADER: &str = "static inline int target(void) { return 1; }\n";

#[test]
fn a_unity_test_of_a_ceedling_project_runs_through_ceedling_by_file_and_case() {
    let repo = scanned_repo(&[
        (
            "project.yml",
            ":project:\n  :build_root: build\n:paths:\n  :test:\n    - test/**\n  :source:\n    - src/**\n",
        ),
        ("src/calc.h", C_HEADER),
        (
            "test/test_calc.c",
            "#include \"unity.h\"\n#include \"calc.h\"\n\nvoid setUp(void) {}\nvoid tearDown(void) {}\n\nvoid test_adds(void) { TEST_ASSERT_EQUAL(1, target()); }\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/calc.h"),
        &[(
            "test_adds",
            "ceedling test:test_calc.c --test-case=test_adds",
        )],
    );
}

#[test]
fn cmocka_and_check_tests_run_through_the_ctest_test_of_their_executable() {
    let files = [
        (
            "CMakeLists.txt",
            "cmake_minimum_required(VERSION 3.20)\nproject(calc C)\nenable_testing()\nadd_subdirectory(tests)\n",
        ),
        (
            "tests/CMakeLists.txt",
            "add_executable(calc_cmocka cmocka_calc.c ../src/calc.c)\nadd_test(NAME calc.cmocka COMMAND calc_cmocka)\n\nadd_executable(calc_check\n    check_calc.c # the suite\n    ${CMAKE_CURRENT_SOURCE_DIR}/../src/calc.c)\nadd_test(calc_check $<TARGET_FILE:calc_check>)\n",
        ),
        ("src/calc.h", C_HEADER),
        (
            "tests/cmocka_calc.c",
            "#include <stdarg.h>\n#include <stddef.h>\n#include <setjmp.h>\n#include <cmocka.h>\n#include \"calc.h\"\n\nstatic void adds(void **state) { assert_int_equal(target(), 1); }\n\nint main(void) {\n    const struct CMUnitTest tests[] = { cmocka_unit_test(adds) };\n    return cmocka_run_group_tests(tests, NULL, NULL);\n}\n",
        ),
        (
            "tests/check_calc.c",
            "#include <check.h>\n#include \"calc.h\"\n\nSTART_TEST(test_check_adds)\n{\n    ck_assert_int_eq(target(), 1);\n}\nEND_TEST\n\nSuite *calc_suite(void) {\n    Suite *s = suite_create(\"Calc\");\n    TCase *tc_core = tcase_create(\"Core\");\n    tcase_add_test(tc_core, test_check_adds);\n    suite_add_tcase(s, tc_core);\n    return s;\n}\n",
        ),
    ];
    let built: Vec<(&str, &str)> = files
        .iter()
        .copied()
        .chain([("build/CTestTestfile.cmake", "subdirs(tests)\n")])
        .collect();
    let configured = scanned_repo(&built);
    let unbuilt = scanned_repo(&files);

    assert_commands(
        &report(&configured, "target", "src/calc.h"),
        &[
            (
                "test_check_adds",
                "CK_RUN_CASE=Core ctest --test-dir build -R '^calc_check$'",
            ),
            (
                "adds",
                "CMOCKA_TEST_FILTER=adds ctest --test-dir build -R '^calc\\.cmocka$'",
            ),
        ],
    );
    assert_commands(
        &report(&unbuilt, "target", "src/calc.h"),
        &[("test_check_adds", "-"), ("adds", "-")],
    );
}

#[test]
fn a_gdunit4_suite_runs_through_its_runtest_script() {
    let repo = scanned_repo(&[
        ("game/project.godot", "config_version=5\n"),
        ("game/addons/gdUnit4/runtest.sh", "#!/usr/bin/env bash\n"),
        (
            "game/test/test_calc.gd",
            "class_name CalcTest\nextends GdUnitTestSuite\n\nfunc target():\n\treturn 1\n\nfunc test_adds() -> void:\n\tassert_int(target()).is_equal(1)\n",
        ),
    ]);
    let without_addon = scanned_repo(&[
        ("game/project.godot", "config_version=5\n"),
        ("game/addons/gut/gut_cmdln.gd", "extends SceneTree\n"),
        (
            "game/test/test_calc.gd",
            "class_name CalcTest\nextends GdUnitTestSuite\n\nfunc target():\n\treturn 1\n\nfunc test_adds() -> void:\n\tassert_int(target()).is_equal(1)\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "game/test/test_calc.gd"),
        &[(
            "test_adds",
            "cd game && ./addons/gdUnit4/runtest.sh -a 'res://test/test_calc.gd'",
        )],
    );
    assert_commands(
        &report(&without_addon, "target", "game/test/test_calc.gd"),
        &[("test_adds", "-")],
    );
}

const KOTEST_SPECS: &str = "package demo\n\nimport io.kotest.core.spec.style.DescribeSpec\nimport io.kotest.core.spec.style.FunSpec\nimport io.kotest.core.spec.style.StringSpec\n\nclass LibSpec : StringSpec({\n    \"adds two\" {\n        target() shouldBe 1\n    }\n})\n\nclass LibFunSpec : FunSpec({\n    context(\"math\") {\n        test(\"adds three\") { target() shouldBe 1 }\n    }\n})\n\nclass LibInit : FunSpec() {\n    init {\n        test(\"isn't four\") { target() shouldBe 1 }\n    }\n}\n\nclass LibDescribe : DescribeSpec({\n    describe(\"calc\") {\n        it(\"adds five\") { target() shouldBe 1 }\n    }\n})\n";

#[test]
fn a_kotest_label_runs_through_gradle_by_its_spec_and_test_path() {
    let repo = scanned_repo(&[
        (
            "build.gradle.kts",
            "plugins { kotlin(\"jvm\") }\ndependencies { testImplementation(\"io.kotest:kotest-runner-junit5:6.0.0\") }\n",
        ),
        ("gradlew", "#!/bin/sh\n"),
        ("gradlew.bat", "@echo off\n"),
        ("src/main/kotlin/demo/Lib.kt", KOTLIN_LIB),
        ("src/test/kotlin/demo/LibSpec.kt", KOTEST_SPECS),
    ]);
    let maven = scanned_repo(&[
        ("pom.xml", "<project></project>\n"),
        ("src/main/kotlin/demo/Lib.kt", KOTLIN_LIB),
        ("src/test/kotlin/demo/LibSpec.kt", KOTEST_SPECS),
    ]);
    let tool = if cfg!(windows) {
        "gradlew.bat"
    } else {
        "./gradlew"
    };

    assert_commands(
        &report(&repo, "target", "src/main/kotlin/demo/Lib.kt"),
        &[
            (
                "adds two",
                &format!("{tool} test --tests 'demo.LibSpec.adds two'"),
            ),
            (
                "adds three",
                &format!("{tool} test --tests 'demo.LibFunSpec.math -- adds three'"),
            ),
            (
                "isn't four",
                &format!("{tool} test --tests 'demo.LibInit.isn'\\''t four'"),
            ),
            (
                "adds five",
                &format!("{tool} test --tests 'demo.LibDescribe.calc -- adds five'"),
            ),
        ],
    );
    assert_commands(
        &report(&maven, "target", "src/main/kotlin/demo/Lib.kt"),
        &[
            ("adds two", "-"),
            ("adds three", "-"),
            ("isn't four", "-"),
            ("adds five", "-"),
        ],
    );
}

#[test]
fn an_munit_test_runs_through_sbt_with_an_escaped_tests_filter() {
    let repo = scanned_repo(&[
        (
            "build.sbt",
            "libraryDependencies += \"org.scalameta\" %% \"munit\" % \"1.0.0\" % Test\n",
        ),
        ("src/main/scala/demo/Lib.scala", SCALA_LIB),
        (
            "src/test/scala/demo/LibSuite.scala",
            "package demo\n\nclass LibSuite extends munit.FunSuite {\n  test(\"adds two\") {\n    assertEquals(Lib.target(), 1)\n  }\n  test(\"adds (two) + 1\".tag(Slow)) {\n    assertEquals(Lib.target(), 1)\n  }\n}\n",
        ),
    ]);

    assert_commands(
        &report(&repo, "target", "src/main/scala/demo/Lib.scala"),
        &[
            (
                "adds two",
                r#"sbt 'testOnly *LibSuite -- "--tests=adds two"'"#,
            ),
            (
                "adds (two) + 1",
                r#"sbt 'testOnly *LibSuite -- "--tests=adds \\(two\\) \\+ 1"'"#,
            ),
        ],
    );
}

const RUBY_TEST_UNIT: &str = "require \"test/unit\"\nrequire \"calc\"\n\nclass TestCalc < Test::Unit::TestCase\n  def test_adds\n    assert_equal 1, target\n  end\n\n  test \"adds two\" do\n    assert_equal 1, target\n  end\n\n  sub_test_case \"nested\" do\n    test \"adds three\" do\n      assert_equal 1, target\n    end\n  end\nend\n";

#[test]
fn a_test_unit_test_runs_through_ruby_by_its_method_name() {
    let bundled = scanned_repo(&[
        (
            "Gemfile",
            "source \"https://rubygems.org\"\ngem \"test-unit\"\n",
        ),
        ("lib/calc.rb", RUBY_LIB),
        ("test/test_calc.rb", RUBY_TEST_UNIT),
    ]);
    let rakefile = scanned_repo(&[
        ("Rakefile", "task default: :test\n"),
        ("lib/calc.rb", RUBY_LIB),
        ("test/test_calc.rb", RUBY_TEST_UNIT),
    ]);
    let unbundled = scanned_repo(&[
        ("Gemfile", "source \"https://rubygems.org\"\ngem \"rake\"\n"),
        ("lib/calc.rb", RUBY_LIB),
        ("test/test_calc.rb", RUBY_TEST_UNIT),
    ]);

    assert_commands(
        &report(&bundled, "target", "lib/calc.rb"),
        &[
            (
                "test_adds",
                "bundle exec ruby -Ilib -Itest test/test_calc.rb '--name=test_adds'",
            ),
            (
                "adds two",
                "bundle exec ruby -Ilib -Itest test/test_calc.rb '--name=test: adds two'",
            ),
            (
                "adds three",
                "bundle exec ruby -Ilib -Itest test/test_calc.rb '--name=test: adds three'",
            ),
        ],
    );
    assert_commands(
        &report(&rakefile, "target", "lib/calc.rb"),
        &[
            (
                "test_adds",
                "ruby -Ilib -Itest test/test_calc.rb '--name=test_adds'",
            ),
            (
                "adds two",
                "ruby -Ilib -Itest test/test_calc.rb '--name=test: adds two'",
            ),
            (
                "adds three",
                "ruby -Ilib -Itest test/test_calc.rb '--name=test: adds three'",
            ),
        ],
    );
    assert_commands(
        &report(&unbundled, "target", "lib/calc.rb"),
        &[("test_adds", "-"), ("adds two", "-"), ("adds three", "-")],
    );
}

#[test]
fn a_zig_test_is_listed_without_a_command() {
    let repo = scanned_repo(&[(
        "src/calc.zig",
        "const std = @import(\"std\");\n\nfn target() i32 {\n    return 1;\n}\n\ntest \"adds two\" {\n    try std.testing.expectEqual(@as(i32, 1), target());\n}\n",
    )]);

    assert_commands(
        &report(&repo, "target", "src/calc.zig"),
        &[("adds two", "-")],
    );
}
