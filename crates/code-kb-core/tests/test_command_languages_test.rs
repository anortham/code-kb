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
            ("addsTwo", "mvn test -Dtest=LibJUnitTest#addsTwo"),
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
