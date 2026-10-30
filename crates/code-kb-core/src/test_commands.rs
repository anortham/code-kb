//! The shell command that runs one test, from the marker files of the project.

mod gradle;

use gradle::GradleSettings;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A class that encloses a test. `test_case` says it derives from `unittest.TestCase`.
pub struct TestClass<'a> {
    pub name: &'a str,
    pub test_case: bool,
}

/// A test whose command is wanted. `path` is relative to the project root and uses `/`. `kind` is
/// the symbol kind (`function` or `method`) and `line` its first line. `classes` are the classes
/// that enclose the test, outermost first. `groups` are the labelled test containers between the
/// innermost class and the test, outermost first, such as a Kotest `context("math")`.
pub struct TestSpec<'a> {
    pub language: &'a str,
    pub kind: &'a str,
    pub path: &'a str,
    pub line: usize,
    pub name: &'a str,
    pub classes: &'a [TestClass<'a>],
    /// The module or namespace that holds the test directly, such as an F# module.
    pub module: Option<&'a str>,
    pub groups: &'a [&'a str],
}

/// Finds test commands. Runner markers and pytest configuration are read at most once per folder
/// for the life of the finder; project-file entries for .NET are listed once. A Rust or Go test
/// file is read at most once for the same time.
pub struct CommandFinder {
    root: PathBuf,
    folders: HashMap<String, Folder>,
    sources: HashMap<String, Vec<String>>,
}

#[derive(Clone, Copy)]
enum Runner {
    Vitest,
    Jest,
    Mocha,
    NodeTest,
}

#[derive(Clone, Copy)]
enum Build {
    Maven,
    Gradle,
    Sbt,
}

#[derive(Clone, Copy)]
enum DartRunner {
    Dart,
    Flutter,
}

/// The Ruby runners a folder with a `Gemfile`, `.rspec`, `Rakefile`, or `bin/rails` configures.
#[derive(Clone)]
struct Ruby {
    gemfile: bool,
    rspec: bool,
    rails: bool,
    rake_test_task: bool,
    test_unit: bool,
}

/// A folder with a `project.godot`, and the test add-ons it has.
#[derive(Clone)]
struct Godot {
    gut: bool,
    gdunit4: bool,
}

/// A folder with a `build.zig`. `filter_option` says it declares a `test` step and a
/// `test-filter` option, so `zig build test -Dtest-filter=…` selects tests.
#[derive(Clone, Copy)]
struct ZigBuild {
    filter_option: bool,
}

/// What CTest needs from a `CMakeLists.txt`: whether it enables testing, the sources of each
/// executable target, and the target that each test runs.
#[derive(Default)]
struct CMakeTests {
    enables_testing: bool,
    sources: Vec<(String, String)>,
    tests: Vec<(String, String)>,
}

/// A folder with a `composer.json` or a PHPUnit configuration file.
#[derive(Clone)]
struct Php {
    composer: bool,
    phpunit: bool,
}

#[derive(Clone, Copy)]
enum TestFamily {
    Rust,
    Go,
    Script,
    Jvm,
    Python,
    Dotnet,
}

impl TestFamily {
    fn for_path(path: &str) -> Option<Self> {
        match Path::new(path)
            .extension()?
            .to_str()?
            .to_ascii_lowercase()
            .as_str()
        {
            "rs" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "vue" => {
                Some(Self::Script)
            }
            "java" | "kt" | "kts" | "scala" | "sc" => Some(Self::Jvm),
            "py" | "pyw" => Some(Self::Python),
            "cs" | "csx" | "fs" | "fsi" | "fsx" | "vb" | "razor" | "cshtml" => Some(Self::Dotnet),
            _ => None,
        }
    }

    fn root_command(self, folder: &Folder, gradle: Option<&str>) -> Option<String> {
        match self {
            Self::Rust => folder.cargo.as_ref().and_then(|cargo| {
                if cargo.workspace {
                    Some("cargo test --workspace".to_string())
                } else {
                    cargo.package.as_ref().map(|_| "cargo test".to_string())
                }
            }),
            Self::Go => folder.go_mod.then(|| "go test ./...".to_string()),
            Self::Script => folder
                .package_json
                .as_ref()
                .filter(|package| package.has_test_script)
                .map(|_| "npm test".to_string()),
            Self::Jvm => {
                if folder.jvm_build_ambiguous {
                    None
                } else {
                    folder.build.map(|build| match build {
                        Build::Maven => "mvn test".to_string(),
                        Build::Gradle => gradle.unwrap_or("gradle test").to_string(),
                        Build::Sbt => "sbt test".to_string(),
                    })
                }
            }
            Self::Python => folder.pytest_config.then(|| "pytest".to_string()),
            Self::Dotnet => folder.dotnet_test_scope.then(|| "dotnet test".to_string()),
        }
    }

    /// The broader command of the package in `folder`: `Some(None)` when the folder bounds the
    /// search but configures no command. `gradle` is the Gradle command of the folder, `None`
    /// when Gradle cannot name its project with certainty.
    fn scoped_command(self, folder: &Folder, gradle: Option<&str>) -> Option<Option<String>> {
        match self {
            Self::Rust => folder.cargo.as_ref().map(|cargo| {
                cargo.package.as_ref().map(|_| {
                    if cargo.workspace {
                        "cargo test --workspace".to_string()
                    } else {
                        "cargo test".to_string()
                    }
                })
            }),
            Self::Go => folder.go_mod.then(|| Some("go test ./...".to_string())),
            Self::Script => folder
                .package_json
                .as_ref()
                .map(|package| package.has_test_script.then(|| "npm test".to_string())),
            Self::Jvm => {
                if folder.jvm_build_ambiguous {
                    Some(None)
                } else {
                    folder.build.map(|build| match build {
                        Build::Maven => Some("mvn test".to_string()),
                        Build::Gradle => gradle.map(str::to_string),
                        Build::Sbt => Some("sbt test".to_string()),
                    })
                }
            }
            Self::Python => {
                if folder.pytest_config {
                    Some(Some("pytest".to_string()))
                } else {
                    folder.python_project_marker.then_some(None)
                }
            }
            Self::Dotnet => {
                if folder.dotnet_test_scope {
                    Some(Some("dotnet test".to_string()))
                } else {
                    folder.dotnet_project_marker.then_some(None)
                }
            }
        }
    }
}

/// The folder Gradle runs from, its program as seen from there, and the test task to run.
struct GradleRun {
    dir: String,
    tool: String,
    task: String,
}

#[derive(Clone)]
struct Cargo {
    package: Option<String>,
    workspace: bool,
}

#[derive(Clone)]
struct PackageJson {
    runner: Option<Runner>,
    has_test_script: bool,
}

struct Folder {
    cargo: Option<Cargo>,
    go_mod: bool,
    package_json: Option<PackageJson>,
    build: Option<Build>,
    jvm_build_ambiguous: bool,
    gradlew: bool,
    gradlew_bat: bool,
    gradle_settings: Option<GradleSettings>,
    pytest_config: bool,
    python_project_marker: bool,
    dotnet_project_marker: bool,
    dotnet_test_scope: bool,
    sbt_scalatest: bool,
    sbt_munit: bool,
    ruby: Option<Ruby>,
    swift_package: bool,
    dart: Option<Option<DartRunner>>,
    cmake_gtest: bool,
    cmake_tests: Option<CMakeTests>,
    ctest_build_dir: Option<String>,
    ceedling_prefix: Option<String>,
    zig: Option<ZigBuild>,
    php: Option<Php>,
    mix: bool,
    rebar: bool,
    busted: Option<bool>,
    testthat: Option<Option<String>>,
    godot: Option<Godot>,
}

impl Folder {
    fn read(dir: &Path) -> Self {
        let text = |name: &str| {
            fs::read(dir.join(name))
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let has = |name: &str| dir.join(name).is_file();
        let pyproject = text("pyproject.toml");
        let setup_cfg = text("setup.cfg");
        let tox_ini = text("tox.ini");
        let pytest_config = has("pytest.ini")
            || pyproject.as_deref().is_some_and(|content| {
                content
                    .lines()
                    .any(|line| line.trim() == "[tool.pytest.ini_options]")
            })
            || setup_cfg
                .as_deref()
                .is_some_and(|content| content.lines().any(|line| line.trim() == "[tool:pytest]"))
            || tox_ini
                .as_deref()
                .is_some_and(|content| content.lines().any(|line| line.trim() == "[pytest]"));
        let python_project_marker = pyproject.as_deref().is_some_and(|content| {
            content
                .lines()
                .any(|line| matches!(line.trim(), "[project]" | "[tool.poetry]"))
        }) || setup_cfg
            .as_deref()
            .is_some_and(|content| content.lines().any(|line| line.trim() == "[metadata]"))
            || has("setup.py");
        let maven = has("pom.xml");
        let gradle = has("build.gradle") || has("build.gradle.kts");
        let build_sbt = text("build.sbt");
        let cmake = text("CMakeLists.txt");
        let mut dotnet_project_marker = false;
        let mut dotnet_test_scope = false;
        let mut rockspec = None;
        let mut ctest_build_dirs = Vec::new();
        let mut gemspec_test_unit = false;
        for entry in fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if cmake.is_some() && entry.path().join("CTestTestfile.cmake").is_file() {
                ctest_build_dirs.push(entry.file_name().to_string_lossy().into_owned());
            }
            if name.ends_with(".gemspec") {
                gemspec_test_unit |= fs::read_to_string(entry.path())
                    .is_ok_and(|source| source.contains("test-unit"));
            } else if name.ends_with(".rockspec") {
                let mentions_busted =
                    fs::read_to_string(entry.path()).is_ok_and(|source| source.contains("busted"));
                rockspec = Some(rockspec.unwrap_or(false) || mentions_busted);
            } else if is_dotnet_solution_marker(&name) {
                dotnet_test_scope = true;
            } else if is_dotnet_project_marker(&name) {
                dotnet_project_marker = true;
                if !dotnet_test_scope
                    && fs::read_to_string(entry.path())
                        .is_ok_and(|source| is_dotnet_test_project(&source))
                {
                    dotnet_test_scope = true;
                }
            }
        }
        let gemfile = text("Gemfile");
        let rails = has("bin/rails");
        let rakefile = text("Rakefile");
        let ruby =
            (gemfile.is_some() || rakefile.is_some() || has(".rspec") || rails).then(|| Ruby {
                gemfile: gemfile.is_some(),
                rspec: has(".rspec")
                    || gemfile
                        .as_deref()
                        .is_some_and(|text| text.contains("rspec")),
                rails,
                rake_test_task: rakefile
                    .as_deref()
                    .is_some_and(|text| text.contains("TestTask")),
                test_unit: gemspec_test_unit
                    || gemfile
                        .as_deref()
                        .is_some_and(|text| text.contains("test-unit")),
            });
        let composer = text("composer.json");
        let phpunit_config = ["phpunit.xml", "phpunit.xml.dist", "phpunit.dist.xml"]
            .iter()
            .any(|name| has(name));
        let php = (composer.is_some() || phpunit_config).then(|| Php {
            composer: composer.is_some(),
            phpunit: phpunit_config
                || composer
                    .as_deref()
                    .is_some_and(|text| text.contains("phpunit/phpunit")),
        });
        let busted = if has(".busted") { Some(true) } else { rockspec };
        let godot = has("project.godot").then(|| Godot {
            gut: has("addons/gut/gut_cmdln.gd"),
            gdunit4: has("addons/gdUnit4/runtest.sh"),
        });
        let ceedling_prefix = text("project.yml")
            .filter(|yaml| yaml.lines().any(|line| line.trim_end() == ":project:"))
            .or_else(|| text("ceedling.yml"))
            .map(|yaml| ceedling_test_prefix(&yaml));
        let zig = text("build.zig").map(|build| ZigBuild {
            filter_option: build.contains("\"test-filter\"") && build.contains("step(\"test\""),
        });
        Folder {
            cargo: text("Cargo.toml").map(|toml| Cargo {
                package: package_name(&toml),
                workspace: has_workspace(&toml),
            }),
            go_mod: has("go.mod"),
            package_json: text("package.json").map(|json| PackageJson {
                runner: runner(&json),
                has_test_script: has_test_script(&json),
            }),
            build: if maven {
                Some(Build::Maven)
            } else if gradle {
                Some(Build::Gradle)
            } else if build_sbt.is_some() {
                Some(Build::Sbt)
            } else {
                None
            },
            jvm_build_ambiguous: [maven, gradle, build_sbt.is_some()]
                .iter()
                .filter(|found| **found)
                .count()
                > 1,
            gradlew: has("gradlew"),
            gradlew_bat: has("gradlew.bat"),
            gradle_settings: match (text("settings.gradle"), text("settings.gradle.kts")) {
                (Some(settings), None) | (None, Some(settings)) => {
                    Some(GradleSettings::parse(&settings))
                }
                (Some(_), Some(_)) => Some(GradleSettings::unreadable()),
                (None, None) => None,
            },
            pytest_config,
            python_project_marker,
            dotnet_project_marker,
            dotnet_test_scope,
            sbt_scalatest: build_sbt
                .as_deref()
                .is_some_and(|text| text.contains("scalatest")),
            sbt_munit: build_sbt
                .as_deref()
                .is_some_and(|text| text.contains("munit")),
            ruby,
            swift_package: has("Package.swift"),
            dart: text("pubspec.yaml").map(|text| dart_runner(&text)),
            cmake_gtest: cmake.as_deref().is_some_and(|text| {
                text.contains("gtest_discover_tests") || text.contains("gtest_add_tests")
            }),
            cmake_tests: cmake.as_deref().map(cmake_tests),
            ceedling_prefix,
            zig,
            ctest_build_dir: (ctest_build_dirs.len() == 1).then(|| ctest_build_dirs.remove(0)),
            php,
            mix: has("mix.exs"),
            rebar: has("rebar.config"),
            busted,
            testthat: text("DESCRIPTION").map(|text| testthat_package(&text)),
            godot,
        }
    }
}

/// The runner a `pubspec.yaml` names: `flutter test` with `flutter_test`, `dart test` with a
/// `test` dependency, else none.
fn dart_runner(pubspec: &str) -> Option<DartRunner> {
    if pubspec.contains("flutter_test") {
        Some(DartRunner::Flutter)
    } else if pubspec
        .lines()
        .any(|line| line.starts_with(char::is_whitespace) && line.trim_start().starts_with("test:"))
    {
        Some(DartRunner::Dart)
    } else {
        None
    }
}

/// The `Package:` of an R `DESCRIPTION` that names `testthat`.
fn testthat_package(description: &str) -> Option<String> {
    if !description.contains("testthat") {
        return None;
    }
    description.lines().find_map(|line| {
        line.strip_prefix("Package:")
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string)
    })
}

/// The `:test_file_prefix:` of a Ceedling project file, `test_` by default.
fn ceedling_test_prefix(yaml: &str) -> String {
    yaml.lines()
        .find_map(|line| line.trim().strip_prefix(":test_file_prefix:"))
        .map(|value| value.trim().trim_matches(['"', '\'']).to_string())
        .filter(|prefix| !prefix.is_empty())
        .unwrap_or_else(|| "test_".to_string())
}

/// The commands of a CMake file with their arguments, the name lowercased. Comments end at the
/// line end; a quoted argument keeps its spaces and loses its quotes.
fn cmake_commands(text: &str) -> Vec<(String, Vec<String>)> {
    let mut commands = Vec::new();
    let mut chars = text.chars();
    let mut word = String::new();
    let mut after_word = false;
    while let Some(c) = chars.next() {
        if c == '#' {
            chars.by_ref().find(|c| *c == '\n');
            word.clear();
        } else if c.is_ascii_alphanumeric() || c == '_' {
            if after_word {
                word.clear();
                after_word = false;
            }
            word.push(c);
        } else if c == '(' && !word.is_empty() {
            let name = std::mem::take(&mut word).to_ascii_lowercase();
            commands.push((name, cmake_arguments(&mut chars)));
            after_word = false;
        } else if c.is_whitespace() {
            after_word = !word.is_empty();
        } else {
            word.clear();
            after_word = false;
        }
    }
    commands
}

/// The arguments of a CMake command up to its closing parenthesis.
fn cmake_arguments(chars: &mut std::str::Chars) -> Vec<String> {
    let mut arguments = Vec::new();
    let mut argument = String::new();
    let mut depth = 0;
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                for quoted in chars.by_ref() {
                    if quoted == '"' {
                        break;
                    }
                    argument.push(quoted);
                }
            }
            '#' => {
                chars.by_ref().find(|c| *c == '\n');
            }
            '(' => {
                depth += 1;
                argument.push(c);
            }
            ')' if depth == 0 => break,
            ')' => {
                depth -= 1;
                argument.push(c);
            }
            c if c.is_whitespace() => {
                if !argument.is_empty() {
                    arguments.push(std::mem::take(&mut argument));
                }
            }
            c => argument.push(c),
        }
    }
    if !argument.is_empty() {
        arguments.push(argument);
    }
    arguments
}

/// The `add_executable`, `target_sources`, `add_test`, `enable_testing`, and `include(CTest)`
/// calls of a `CMakeLists.txt`.
fn cmake_tests(text: &str) -> CMakeTests {
    const KEYWORDS: [&str; 6] = [
        "WIN32",
        "MACOSX_BUNDLE",
        "EXCLUDE_FROM_ALL",
        "PRIVATE",
        "PUBLIC",
        "INTERFACE",
    ];
    let mut tests = CMakeTests::default();
    for (command, arguments) in cmake_commands(text) {
        match (command.as_str(), arguments.as_slice()) {
            ("enable_testing", _) => tests.enables_testing = true,
            ("include", [module, ..]) if module == "CTest" => tests.enables_testing = true,
            ("add_executable" | "target_sources", [target, sources @ ..]) => {
                if sources
                    .iter()
                    .any(|source| matches!(source.as_str(), "IMPORTED" | "ALIAS"))
                {
                    continue;
                }
                tests.sources.extend(
                    sources
                        .iter()
                        .filter(|source| !KEYWORDS.contains(&source.as_str()))
                        .map(|source| (target.clone(), source.clone())),
                );
            }
            ("add_test", [keyword, name, rest @ ..]) if keyword == "NAME" => {
                if let Some(at) = rest.iter().position(|word| word == "COMMAND")
                    && let Some(program) = rest.get(at + 1)
                {
                    tests.tests.push((name.clone(), program.clone()));
                }
            }
            ("add_test", [name, program, ..]) => {
                tests.tests.push((name.clone(), program.clone()));
            }
            _ => {}
        }
    }
    tests
}

/// The target that a CTest command runs: the program itself, or the target of
/// `$<TARGET_FILE:target>`.
fn cmake_test_target(program: &str) -> &str {
    program
        .strip_prefix("$<TARGET_FILE:")
        .and_then(|rest| rest.strip_suffix('>'))
        .unwrap_or(program)
}

/// `source` of a CMake file in `dir` as a path below the project root, or `None` when it names a
/// variable other than the current source folder or leaves the root.
fn cmake_source_path(dir: &str, source: &str) -> Option<String> {
    let source = ["${CMAKE_CURRENT_SOURCE_DIR}/", "${CMAKE_CURRENT_LIST_DIR}/"]
        .iter()
        .find_map(|prefix| source.strip_prefix(prefix))
        .unwrap_or(source);
    if source.contains("${") || source.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in source.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// `text` as a CMake regular expression that matches it literally.
fn cmake_regex_escaped(text: &str) -> String {
    backslashed(text, |c| "^$.[]*+?()|\\".contains(c))
}

/// The `name` key of the `[package]` table.
pub(crate) fn package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            return value.split(['"', '\'']).nth(1).map(str::to_string);
        }
    }
    None
}

fn has_workspace(toml: &str) -> bool {
    toml.lines()
        .map(str::trim)
        .any(|line| line == "[workspace]" || line.starts_with("[workspace."))
}

fn is_dotnet_solution_marker(name: &str) -> bool {
    [".sln", ".slnx"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn is_dotnet_project_marker(name: &str) -> bool {
    [".csproj", ".fsproj", ".vbproj"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn is_dotnet_test_project(source: &str) -> bool {
    let normalized = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.contains("<istestproject>false</istestproject>") {
        return false;
    }
    normalized.contains("<istestproject>true</istestproject>")
        || normalized.contains("microsoft.net.test.sdk")
}

/// The Gradle program for the build in `dir`: the nearest wrapper at or above it, given the
/// folders that hold `gradlew` and `gradlew.bat`, else `gradle`. Windows takes `gradlew.bat` when
/// there is one.
fn gradle_tool(
    windows: bool,
    dir: &str,
    gradlew: Option<&str>,
    gradlew_bat: Option<&str>,
) -> String {
    match (windows, gradlew_bat, gradlew) {
        (true, Some(wrapper_dir), _) => match depth(dir) - depth(wrapper_dir) {
            0 => "gradlew.bat".to_string(),
            up => format!("{}gradlew.bat", "..\\".repeat(up)),
        },
        (_, _, Some(wrapper_dir)) => wrapper_path(dir, wrapper_dir),
        _ => "gradle".to_string(),
    }
}

#[cfg(test)]
fn gradle_test_command(
    windows: bool,
    dir: &str,
    gradlew: Option<&str>,
    gradlew_bat: Option<&str>,
) -> String {
    format!("{} test", gradle_tool(windows, dir, gradlew, gradlew_bat))
}

fn runner(package_json: &str) -> Option<Runner> {
    let json: serde_json::Value = serde_json::from_str(package_json).ok()?;
    let script = json["scripts"]["test"].as_str().unwrap_or_default();
    let uses = |name: &str| {
        ["devDependencies", "dependencies"]
            .iter()
            .any(|section| json[*section].get(name).is_some())
            || script.split_whitespace().any(|word| word == name)
    };
    if uses("vitest") {
        Some(Runner::Vitest)
    } else if uses("jest") {
        Some(Runner::Jest)
    } else if uses("mocha") {
        Some(Runner::Mocha)
    } else if script.contains("node --test") {
        Some(Runner::NodeTest)
    } else {
        None
    }
}

fn has_test_script(package_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(package_json)
        .ok()
        .and_then(|json| {
            json["scripts"]["test"]
                .as_str()
                .map(str::trim)
                .map(str::to_string)
        })
        .is_some_and(|script| !script.is_empty() && !is_npm_placeholder_script(&script))
}

/// Whether `script` is the test script that `npm init` writes, which runs no test and fails.
fn is_npm_placeholder_script(script: &str) -> bool {
    script.contains("no test specified") && script.trim_end().ends_with("exit 1")
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn relative<'a>(dir: &str, path: &'a str) -> &'a str {
    if dir.is_empty() {
        path
    } else {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(path)
    }
}

fn backslashed(name: &str, special: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if special(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `name` made safe inside a double-quoted shell word.
fn double_quoted(name: &str) -> String {
    backslashed(name, |c| matches!(c, '"' | '\\' | '$' | '`'))
}

fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `text` as one shell word: unchanged when it holds only `A-Z a-z 0-9 _ . / -`, else
/// single-quoted.
fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'));
    if plain {
        text.to_string()
    } else {
        single_quoted(text)
    }
}

/// `command` run from the folder `dir` below the project root.
fn in_folder(dir: &str, command: String) -> String {
    if dir.is_empty() {
        command
    } else {
        format!("cd {} && {command}", shell_word(dir))
    }
}

/// Stands for a character that a command line cannot carry: NUL, or half of a surrogate pair.
const UNREPRESENTABLE: char = '\0';

/// A JavaScript test name as the string its source literal holds. julie gives the literal text, so
/// `\'` becomes `'`, `\\` becomes `\`, and `\n`, `\x41`, `\u0041`, `\u{41}`, and `\101` become
/// the character they name. A backslash before a line break is a line continuation: both vanish.
/// A character a command line cannot carry becomes NUL.
fn js_unescaped(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(escaped) = rest.chars().next() else {
            out.push('\\');
            break;
        };
        if let Some(len) = line_break_len(rest) {
            rest = &rest[len..];
            continue;
        }
        let (decoded, len) = decoded_escape(rest).unwrap_or((escaped, escaped.len_utf8()));
        out.push(decoded);
        rest = &rest[len..];
    }
    out
}

/// The length of the line break that starts `text`: `\n`, `\r\n`, `\r`, U+2028, or U+2029.
fn line_break_len(text: &str) -> Option<usize> {
    if text.starts_with("\r\n") {
        return Some(2);
    }
    text.chars()
        .next()
        .filter(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
        .map(char::len_utf8)
}

/// The character that the escape after a backslash names, with the length of the escape in bytes.
/// `None` for an escape that names itself or is malformed.
fn decoded_escape(text: &str) -> Option<(char, usize)> {
    match text.as_bytes().first()? {
        b'n' => Some(('\n', 1)),
        b't' => Some(('\t', 1)),
        b'r' => Some(('\r', 1)),
        b'b' => Some(('\u{8}', 1)),
        b'f' => Some(('\u{c}', 1)),
        b'v' => Some(('\u{b}', 1)),
        b'0'..=b'3' => octal_escape(text, 3),
        b'4'..=b'7' => octal_escape(text, 2),
        b'x' => Some((char::from_u32(hex_value(text.get(1..3)?)?)?, 3)),
        b'u' => unicode_escape(text),
        _ => None,
    }
}

fn octal_escape(text: &str, max_digits: usize) -> Option<(char, usize)> {
    let len = text
        .bytes()
        .take(max_digits)
        .take_while(|byte| matches!(byte, b'0'..=b'7'))
        .count();
    Some((
        char::from_u32(u32::from_str_radix(&text[..len], 8).ok()?)?,
        len,
    ))
}

fn hex_value(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `text` starts with the `u` of `\uHHHH` or `\u{H…}`. A pair of `\u` surrogates is one character.
fn unicode_escape(text: &str) -> Option<(char, usize)> {
    if let Some(braced) = text.strip_prefix("u{") {
        let digits = &braced[..braced.find('}')?];
        let code = hex_value(digits).filter(|code| *code <= 0x10FFFF)?;
        let character = char::from_u32(code).unwrap_or(UNREPRESENTABLE);
        return Some((character, digits.len() + 3));
    }
    let high = hex_value(text.get(1..5)?)?;
    let low = text
        .get(5..)
        .and_then(|after| after.strip_prefix("\\u"))
        .and_then(|after| hex_value(after.get(..4)?))
        .filter(|low| (0xDC00..=0xDFFF).contains(low));
    match (high, low) {
        (0xD800..=0xDBFF, Some(low)) => {
            let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            Some((char::from_u32(code)?, 11))
        }
        _ => Some((char::from_u32(high).unwrap_or(UNREPRESENTABLE), 5)),
    }
}

/// The length of the `${…}` hole of a template literal that starts `text`, up to its matching `}`.
/// A string or a template inside the hole may hold a `}`.
fn hole_len(text: &str) -> Option<usize> {
    let code = text.strip_prefix("${")?;
    Some(2 + closing_brace(code.as_bytes())? + 1)
}

/// The index of the `}` that closes the code in `code`, which starts after a `{`.
fn closing_brace(code: &[u8]) -> Option<usize> {
    let mut depth = 0;
    let mut at = 0;
    while at < code.len() {
        match code[at] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(at),
            b'}' => depth -= 1,
            b'\'' | b'"' | b'`' => {
                at = after_literal(code, at)?;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The index after the string or template literal that starts at `start`. A template may hold
/// holes of its own.
fn after_literal(code: &[u8], start: usize) -> Option<usize> {
    let quote = code[start];
    let mut at = start + 1;
    while at < code.len() {
        match code[at] {
            b'\\' => at += 1,
            b'$' if quote == b'`' && code.get(at + 1) == Some(&b'{') => {
                at += 2 + closing_brace(&code[at + 2..])?;
            }
            byte if byte == quote => return Some(at + 1),
            _ => {}
        }
        at += 1;
    }
    None
}

/// The text of a JavaScript test name between its `${…}` holes, decoded by `js_unescaped`. A
/// character a command line cannot carry ends a part like a hole does.
fn literal_parts(name: &str) -> Vec<String> {
    let bytes = name.as_bytes();
    let mut raw_parts = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            at += 2;
        } else if bytes[at] == b'$'
            && let Some(len) = hole_len(&name[at..])
        {
            raw_parts.push(&name[start..at]);
            at += len;
            start = at;
        } else {
            at += 1;
        }
    }
    raw_parts.push(&name[start..]);
    raw_parts
        .into_iter()
        .flat_map(|raw| {
            js_unescaped(raw)
                .split(UNREPRESENTABLE)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn push_regex_escaped(out: &mut String, c: char) {
    match c {
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
        c if c.is_control() => out.push_str(&format!("\\x{:02x}", u32::from(c))),
        '\\' | '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => {
            out.push('\\');
            out.push(c);
        }
        _ => out.push(c),
    }
}

fn is_word_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'_'
}

/// The length of the `test.each` placeholder that starts `text`: `%s %d %i %f %j %o %O %p %#`, or
/// `$name` with optional `.path` segments.
fn placeholder_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    match bytes {
        [
            b'%',
            b's' | b'd' | b'i' | b'f' | b'j' | b'o' | b'O' | b'p' | b'#',
            ..,
        ] => Some(2),
        [b'$', first, ..] if first.is_ascii_alphabetic() || *first == b'_' => {
            let mut len = 1;
            loop {
                len += bytes[len..].iter().take_while(|b| is_word_byte(b)).count();
                match &bytes[len..] {
                    [b'.', next, ..] if is_word_byte(next) => len += 1,
                    _ => return Some(len),
                }
            }
        }
        _ => None,
    }
}

/// `name` as the regular expression of `-t`: each placeholder matches any text, `%%` matches one
/// or two percent signs (`test.each` writes one, a plain test writes two), and every other
/// character matches itself.
fn each_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("%%") {
            out.push_str("%%?");
            rest = &rest[2..];
        } else if let Some(len) = placeholder_len(rest) {
            out.push_str(".*");
            rest = &rest[len..];
        } else {
            push_regex_escaped(&mut out, c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The regular expression of `-t` for a name: each hole between its literal parts matches any
/// text.
fn name_pattern(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| each_pattern(part))
        .collect::<Vec<_>>()
        .join(".*")
}

/// A character that would split the command line: a control character other than a tab, or a
/// Unicode line or paragraph separator.
fn splits_a_line(c: char) -> bool {
    (c.is_control() && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}')
}

/// The part of a name that `--fgrep` can match as a substring: the longest piece, between holes
/// and characters that split a line, that holds more than blanks. The first of equal pieces wins.
fn longest_literal(parts: &[String]) -> Option<&str> {
    parts
        .iter()
        .flat_map(|part| part.split(splits_a_line))
        .filter(|piece| !piece.trim().is_empty())
        .rev()
        .max_by_key(|piece| piece.len())
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.split('/').count()
    }
}

/// The path of the Gradle wrapper in `wrapper_dir` as seen from `dir`, a folder at or below it.
fn wrapper_path(dir: &str, wrapper_dir: &str) -> String {
    match depth(dir) - depth(wrapper_dir) {
        0 => "./gradlew".to_string(),
        up => format!("{}gradlew", "../".repeat(up)),
    }
}

/// Whether `go test` reads `name` as a function of the kind `prefix`: the prefix alone, or the
/// prefix followed by anything but a lowercase letter.
fn names_go_test(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.chars().next().is_some_and(char::is_lowercase))
}

/// Whether pytest collects a test in these classes under its defaults: every class is named
/// `Test…` or derives from `unittest.TestCase`, and no class around another is a `TestCase`,
/// because pytest collects only the methods of one.
fn pytest_collects(classes: &[TestClass]) -> bool {
    let named = classes
        .iter()
        .all(|class| class.test_case || class.name.starts_with("Test"));
    let inside_test_case = classes
        .split_last()
        .is_some_and(|(_, outer)| outer.iter().any(|class| class.test_case));
    named && !inside_test_case
}

/// `pytest path::Class::name`. pytest rejects a node id whose path holds `[` and splits one at
/// `::`, so a test in such a folder runs from that folder; a file name with either has no command.
fn python(test: &TestSpec) -> Option<String> {
    if !pytest_collects(test.classes) {
        return None;
    }
    let breaks_a_node_id = |text: &str| text.contains('[') || text.contains("::");
    let (dir, file) = test.path.rsplit_once('/').unwrap_or(("", test.path));
    if breaks_a_node_id(file) {
        return None;
    }
    let (dir, file) = if breaks_a_node_id(dir) {
        (dir, file)
    } else {
        ("", test.path)
    };
    let classes: String = test
        .classes
        .iter()
        .map(|class| format!("{}::", class.name))
        .collect();
    Some(in_folder(
        dir,
        format!("pytest {}::{classes}{}", shell_word(file), test.name),
    ))
}

impl CommandFinder {
    /// A finder for the project at `root`. It never looks above `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            folders: HashMap::new(),
            sources: HashMap::new(),
        }
    }

    /// The command that runs `test`, or `None` when the language or the runner is unknown.
    pub fn command(&mut self, test: &TestSpec) -> Option<String> {
        match test.language {
            "python" => python(test),
            "rust" => Some(self.rust(test)),
            "go" => self.go(test),
            "javascript" | "typescript" | "tsx" | "jsx" => self.script(test),
            "java" | "kotlin" | "scala" => self.jvm(test),
            "csharp" | "fsharp" | "vbnet" => dotnet(test),
            "ruby" => self.ruby(test),
            "swift" => self.swift(test),
            "dart" => self.dart(test),
            "cpp" => self.ctest(test),
            "c" => self.c_test(test),
            "zig" => self.zig(test),
            "qml" => self.qml(test),
            "php" => self.phpunit(test),
            "elixir" => self.mix(test),
            "erlang" => self.rebar(test),
            "lua" => self.busted(test),
            "r" => self.testthat(test),
            "powershell" => pester(test),
            "bash" => bats(test),
            "gdscript" => self.godot(test),
            _ => None,
        }
    }

    /// The configured command for the broader suite nearest to `path`.
    pub fn broader_test_command(&mut self, path: Option<&str>) -> Option<String> {
        if let Some(path) = path {
            let family = TestFamily::for_path(path)?;
            return self
                .nearest_broader_command(parent_dir(path), family)
                .map(|(directory, command)| in_folder(&directory, command));
        }

        if self.folder("").jvm_build_ambiguous {
            return None;
        }
        let gradle = self.gradle_command("");
        let folder = self.folder("");
        let mut command = None;
        for family in [
            TestFamily::Rust,
            TestFamily::Go,
            TestFamily::Script,
            TestFamily::Jvm,
            TestFamily::Python,
            TestFamily::Dotnet,
        ] {
            if let Some(candidate) = family.root_command(folder, gradle.as_deref()) {
                if command.is_some() {
                    return None;
                }
                command = Some(candidate);
            }
        }
        command
    }

    fn nearest_broader_command(
        &mut self,
        folder: &str,
        family: TestFamily,
    ) -> Option<(String, String)> {
        let mut directory = folder;
        loop {
            let gradle = (matches!(family, TestFamily::Jvm)
                && matches!(self.folder(directory).build, Some(Build::Gradle)))
            .then(|| self.gradle_run(directory, folder))
            .flatten();
            let gradle_command = gradle
                .as_ref()
                .map(|run| format!("{} {}", run.tool, run.task));
            if let Some(command) =
                family.scoped_command(self.folder(directory), gradle_command.as_deref())
            {
                let run_dir = gradle.map_or_else(|| directory.to_string(), |run| run.dir);
                return command.map(|command| (run_dir, command));
            }
            if directory.is_empty() {
                return None;
            }
            directory = parent_dir(directory);
        }
    }

    /// The Gradle command for the whole build in `dir`, or `None` when `dir` has none.
    fn gradle_command(&mut self, dir: &str) -> Option<String> {
        if !matches!(self.folder(dir).build, Some(Build::Gradle)) {
            return None;
        }
        Some(format!("{} test", self.gradle_tool(dir)))
    }

    /// How Gradle runs only the tests of the project whose build file is in `build_dir`, for a
    /// test in `folder`. Without a settings file for `build_dir` (`gradle_settings_dir`) that is a
    /// single-project build run from `build_dir`. With one, Gradle runs from the settings folder
    /// with the project's own task (`:a:test`), because a plain `test` in a project folder also
    /// runs the test task of every project below it, where a `--tests` filter matches nothing and
    /// fails. `None` when the settings file cannot name the project with certainty.
    fn gradle_run(&mut self, build_dir: &str, folder: &str) -> Option<GradleRun> {
        let Some(settings_dir) = self.gradle_settings_dir(build_dir)? else {
            return Some(GradleRun {
                tool: self.gradle_tool(build_dir),
                dir: build_dir.to_string(),
                task: "test".to_string(),
            });
        };
        let task = self
            .folder(&settings_dir)
            .gradle_settings
            .as_ref()?
            .test_task(&settings_dir, build_dir, folder)?;
        Some(GradleRun {
            tool: self.gradle_tool(&settings_dir),
            dir: settings_dir,
            task,
        })
    }

    /// The folder of the settings file that defines the build of the build file in `build_dir`.
    /// Gradle takes the nearest settings file at or above the folder it runs from, but does not
    /// look above a folder named `buildSrc`, which is a build of its own. With no settings file
    /// there, a sibling folder whose settings file places a project in `build_dir`
    /// (`includeFlat`) defines it, because Gradle run from `build_dir` would build it alone,
    /// without the projects it can depend on. `Some(None)` when no settings file defines it, and
    /// `None` when two sibling folders do.
    fn gradle_settings_dir(&mut self, build_dir: &str) -> Option<Option<String>> {
        let has_settings = |folder: &Folder| folder.gradle_settings.is_some().then_some(());
        let own_build = build_dir.rsplit('/').next() == Some("buildSrc");
        if own_build {
            return Some(has_settings(self.folder(build_dir)).map(|()| build_dir.to_string()));
        }
        if let Some((settings_dir, ())) = self.nearest(build_dir, has_settings) {
            return Some(Some(settings_dir));
        }
        if build_dir.is_empty() {
            return Some(None);
        }
        let parent = parent_dir(build_dir);
        let Ok(entries) = fs::read_dir(self.root.join(parent)) else {
            return Some(None);
        };
        let mut siblings: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .filter(|name| !name.starts_with('.'))
            .map(|name| {
                if parent.is_empty() {
                    name
                } else {
                    format!("{parent}/{name}")
                }
            })
            .filter(|sibling| sibling != build_dir)
            .collect();
        siblings.sort();
        let mut defining = siblings.into_iter().filter(|sibling| {
            self.folder(sibling)
                .gradle_settings
                .as_ref()
                .is_some_and(|settings| settings.names_dir(sibling, build_dir))
        });
        let settings_dir = defining.next();
        if defining.next().is_some() {
            return None;
        }
        Some(settings_dir)
    }

    /// The Gradle program for the build in `dir` on this platform.
    fn gradle_tool(&mut self, dir: &str) -> String {
        let gradlew = self.nearest(dir, |folder| folder.gradlew.then_some(()));
        let gradlew_bat = self.nearest(dir, |folder| folder.gradlew_bat.then_some(()));
        gradle_tool(
            cfg!(windows),
            dir,
            gradlew
                .as_ref()
                .map(|(wrapper_dir, _)| wrapper_dir.as_str()),
            gradlew_bat
                .as_ref()
                .map(|(wrapper_dir, _)| wrapper_dir.as_str()),
        )
    }

    fn folder(&mut self, dir: &str) -> &Folder {
        let root = &self.root;
        self.folders
            .entry(dir.to_string())
            .or_insert_with(|| Folder::read(&root.join(dir)))
    }

    /// The folder nearest to `dir`, at or above it and not above the root, that `pick` accepts,
    /// with what `pick` found there.
    fn nearest<T>(
        &mut self,
        dir: &str,
        pick: impl Fn(&Folder) -> Option<T>,
    ) -> Option<(String, T)> {
        let mut dir = dir;
        loop {
            if let Some(found) = pick(self.folder(dir)) {
                return Some((dir.to_string(), found));
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent_dir(dir);
        }
    }

    /// The lines of the file `path`, empty when it cannot be read.
    fn source(&mut self, path: &str) -> &[String] {
        let root = &self.root;
        self.sources.entry(path.to_string()).or_insert_with(|| {
            fs::read_to_string(root.join(path))
                .map(|text| text.lines().map(str::to_string).collect())
                .unwrap_or_default()
        })
    }

    fn rust(&mut self, test: &TestSpec) -> String {
        let ignored = if self.has_ignore_attribute(test) {
            " -- --ignored"
        } else {
            ""
        };
        let name = format!("{}{ignored}", test.name);
        let Some((dir, cargo)) = self.nearest(parent_dir(test.path), |folder| folder.cargo.clone())
        else {
            return format!("cargo test {name}");
        };
        let command = match cargo.package {
            None => format!("cargo test {name}"),
            Some(package) => match relative(&dir, test.path)
                .strip_prefix("tests/")
                .and_then(|rest| rest.strip_suffix(".rs"))
                .filter(|stem| !stem.contains('/'))
            {
                Some(stem) => {
                    format!("cargo test -p {package} --test {} {name}", shell_word(stem))
                }
                None => format!("cargo test -p {package} {name}"),
            },
        };
        if self
            .folder("")
            .cargo
            .as_ref()
            .is_some_and(|root| root.workspace)
        {
            command
        } else {
            in_folder(&dir, command)
        }
    }

    /// Whether an attribute above the function says `#[ignore]` or `#[ignore = "…"]`.
    fn has_ignore_attribute(&mut self, test: &TestSpec) -> bool {
        let above = test.line.saturating_sub(1);
        self.source(test.path)
            .iter()
            .take(above)
            .rev()
            .map(|line| line.trim())
            .take_while(|line| line.is_empty() || line.starts_with('#') || line.starts_with("//"))
            .filter(|line| line.starts_with('#'))
            .any(|line| {
                line.match_indices("#[ignore").any(|(at, found)| {
                    matches!(
                        line[at + found.len()..].trim_start().chars().next(),
                        Some(']' | '=')
                    )
                })
            })
    }

    /// `None` for a function that `go test` does not run by name: a method, a name that is not
    /// `Test`, `Benchmark`, `Fuzz`, or `Example` followed by a capital, `TestMain`, a file that is
    /// not `_test.go`, or an example with no `// Output:` comment.
    fn go(&mut self, test: &TestSpec) -> Option<String> {
        let name = test.name;
        if test.kind != "function" || !test.path.ends_with("_test.go") {
            return None;
        }
        let selector = if names_go_test(name, "Benchmark") {
            format!("-bench '^{name}$' -run '^$'")
        } else if (names_go_test(name, "Test") && name != "TestMain")
            || names_go_test(name, "Fuzz")
            || (names_go_test(name, "Example") && self.go_example_prints_output(test))
        {
            format!("-run '^{name}$'")
        } else {
            return None;
        };
        let module = self
            .nearest(parent_dir(test.path), |folder| folder.go_mod.then_some(()))
            .map_or_else(String::new, |(dir, _)| dir);
        let dir = match parent_dir(relative(&module, test.path)) {
            "" => ".",
            dir => dir,
        };
        Some(in_folder(
            &module,
            format!("go test {} {selector}", shell_word(&format!("./{dir}"))),
        ))
    }

    /// Whether the example that starts on `test.line` ends in an `// Output:` comment. `go test`
    /// compiles an example without one and does not run it. The body ends at the first `}` in
    /// column 0, which `gofmt` guarantees.
    fn go_example_prints_output(&mut self, test: &TestSpec) -> bool {
        let mut body = self
            .source(test.path)
            .iter()
            .skip(test.line.saturating_sub(1));
        if body
            .next()
            .is_none_or(|first| first.trim_end().ends_with('}'))
        {
            return false;
        }
        body.take_while(|line| line.as_str() != "}").any(|line| {
            let comment = line.trim().strip_prefix("//").unwrap_or("").trim_start();
            let comment = comment.to_ascii_lowercase();
            comment.starts_with("output:") || comment.starts_with("unordered output:")
        })
    }

    fn script(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, package) =
            self.nearest(parent_dir(test.path), |folder| folder.package_json.clone())?;
        let path = shell_word(relative(&dir, test.path));
        let parts = literal_parts(test.name);
        let pattern = || double_quoted(&name_pattern(&parts));
        let command = match package.runner? {
            Runner::Vitest => format!("npx vitest run {path} -t \"{}\"", pattern()),
            Runner::Jest => format!("npx jest --runTestsByPath {path} -t \"{}\"", pattern()),
            Runner::Mocha => match longest_literal(&parts) {
                Some(part) => format!("npx mocha {path} --fgrep \"{}\"", double_quoted(part)),
                None => format!("npx mocha {path}"),
            },
            Runner::NodeTest => format!(
                "node --test-name-pattern \"{}\" {path}",
                double_quoted(&format!("^{}$", name_pattern(&parts)))
            ),
        };
        Some(in_folder(&dir, command))
    }

    /// A JUnit-style test of Java, Kotlin, or Scala through the nearest Maven, Gradle, or sbt
    /// build. sbt runs the whole class, with MUnit's `--tests` filter for a Scala test label of a
    /// file that names MUnit when `build.sbt` names it, else ScalaTest's `-z` filter when
    /// `build.sbt` names ScalaTest. Gradle selects a Kotest label of a file that imports Kotest by
    /// its test path; otherwise Maven and Gradle select JUnit methods only, so a Kotlin or Scala
    /// test label (a `function` in a class) has no command there. Gradle runs the task of the
    /// test's own project (`gradle_run`).
    fn jvm(&mut self, test: &TestSpec) -> Option<String> {
        if test.classes.is_empty() {
            return None;
        }
        let label = test.language != "java" && test.kind == "function";
        let class = test
            .classes
            .iter()
            .map(|class| class.name)
            .collect::<Vec<_>>()
            .join("$");
        let (dir, build) = self.nearest(parent_dir(test.path), |folder| folder.build)?;
        let kotest = label
            && test.language == "kotlin"
            && matches!(build, Build::Gradle)
            && self.mentions(test.path, "import io.kotest.");
        let munit = label
            && test.language == "scala"
            && matches!(build, Build::Sbt)
            && self.folder(&dir).sbt_munit
            && self.mentions(test.path, "munit");
        let command = match build {
            Build::Gradle if kotest => {
                let run = self.gradle_run(&dir, parent_dir(test.path))?;
                let path = test
                    .groups
                    .iter()
                    .copied()
                    .chain([test.name])
                    .collect::<Vec<_>>()
                    .join(" -- ");
                let filter = match self.kotlin_package(test.path) {
                    Some(package) => format!("{package}.{class}.{path}"),
                    None => format!("{class}.{path}"),
                };
                return Some(in_folder(
                    &run.dir,
                    format!(
                        "{} {} --tests {}",
                        run.tool,
                        run.task,
                        single_quoted(&filter)
                    ),
                ));
            }
            Build::Maven | Build::Gradle if label => return None,
            Build::Maven => format!(
                "mvn test -Dtest={}#{}",
                shell_word(&class),
                shell_word(test.name)
            ),
            Build::Gradle => {
                let run = self.gradle_run(&dir, parent_dir(test.path))?;
                let filter = single_quoted(&format!("{class}.{}", test.name));
                return Some(in_folder(
                    &run.dir,
                    format!("{} {} --tests {filter}", run.tool, run.task),
                ));
            }
            Build::Sbt => {
                let mut task = format!("testOnly *{class}");
                if munit {
                    if !test.name.contains('\\') {
                        let mut pattern = String::from("--tests=");
                        regex_escaped_into(&mut pattern, test.name);
                        let argument = backslashed(&pattern, |c| matches!(c, '"' | '\\'));
                        task.push_str(&format!(" -- \"{argument}\""));
                    }
                } else if label
                    && test.language == "scala"
                    && self.folder(&dir).sbt_scalatest
                    && !test.name.contains(['"', '\\'])
                {
                    task.push_str(&format!(" -- -z \"{}\"", test.name));
                }
                format!("sbt {}", single_quoted(&task))
            }
        };
        Some(in_folder(&dir, command))
    }

    /// RSpec runs a `_spec.rb` example by its line; Rails and a Rake `TestTask` run a Minitest
    /// test; `ruby` runs any other test of a test-unit file by its method name. The nearest
    /// folder with a `Gemfile`, `.rspec`, `Rakefile`, or `bin/rails` decides.
    fn ruby(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, ruby) = self.nearest(parent_dir(test.path), |folder| folder.ruby.clone())?;
        let path = shell_word(relative(&dir, test.path));
        let bundle = if ruby.gemfile { "bundle exec " } else { "" };
        let command = if test.path.ends_with("_spec.rb") {
            if !ruby.rspec {
                return None;
            }
            format!("{bundle}rspec {path}:{}", test.line)
        } else if ruby.rails {
            format!("bin/rails test {path}:{}", test.line)
        } else if ruby.rake_test_task && test.kind == "method" && !test.classes.is_empty() {
            format!(
                "{bundle}rake test TEST={path} TESTOPTS={}",
                shell_word(&format!("--name={}", test.name))
            )
        } else if (!ruby.gemfile || ruby.test_unit) && self.is_test_unit_file(test.path) {
            let method = if test.kind == "function" {
                format!("test: {}", test.name)
            } else {
                test.name.to_string()
            };
            format!(
                "{bundle}ruby -Ilib -Itest {path} {}",
                shell_word(&format!("--name={method}"))
            )
        } else {
            return None;
        };
        Some(in_folder(&dir, command))
    }

    /// Whether the file loads test-unit or defines a `Test::Unit::TestCase`.
    fn is_test_unit_file(&mut self, path: &str) -> bool {
        self.source(path).iter().any(|line| {
            let line = line.trim();
            line.contains("Test::Unit::TestCase")
                || ["require \"test/unit\"", "require 'test/unit'"]
                    .iter()
                    .any(|require| line.starts_with(require))
        })
    }

    /// Whether a line of the file holds `text`.
    fn mentions(&mut self, path: &str, text: &str) -> bool {
        self.source(path).iter().any(|line| line.contains(text))
    }

    /// The package that a Kotlin file declares.
    fn kotlin_package(&mut self, path: &str) -> Option<String> {
        self.source(path).iter().find_map(|line| {
            line.trim()
                .strip_prefix("package ")
                .map(|package| package.trim().trim_end_matches(';').trim().to_string())
                .filter(|package| !package.is_empty())
        })
    }

    /// `swift test --filter` under the nearest `Package.swift`, as a regular expression over
    /// `Target.Class/name` (XCTest) or `Target.Suite/name()` (Swift Testing).
    fn swift(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, ()) = self.nearest(parent_dir(test.path), |folder| {
            folder.swift_package.then_some(())
        })?;
        let mut filter = String::new();
        if test.classes.is_empty() {
            filter.push_str("[./]");
        } else {
            for class in test.classes {
                regex_escaped_into(&mut filter, class.name);
                filter.push('/');
            }
        }
        regex_escaped_into(&mut filter, test.name);
        filter.push_str("\\b");
        Some(in_folder(
            &dir,
            format!("swift test --filter {}", single_quoted(&filter)),
        ))
    }

    /// `dart test` or `flutter test` with `--plain-name`, a substring of the full test name, under
    /// the nearest `pubspec.yaml`. A name with an escape or an interpolation runs the whole file.
    fn dart(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, runner) = self.nearest(parent_dir(test.path), |folder| folder.dart)?;
        let tool = match runner? {
            DartRunner::Dart => "dart test",
            DartRunner::Flutter => "flutter test",
        };
        let path = shell_word(relative(&dir, test.path));
        let command = if test.name.contains(['$', '\\']) {
            format!("{tool} {path}")
        } else {
            format!("{tool} {path} --plain-name {}", single_quoted(test.name))
        };
        Some(in_folder(&dir, command))
    }

    /// `ctest -R` for a GoogleTest `TEST`, `TEST_F`, or `TEST_P` that a `CMakeLists.txt` at or
    /// above the test discovers, in the one CTest build folder below the nearest CMake folder.
    fn ctest(&mut self, test: &TestSpec) -> Option<String> {
        let (suite, case) = test.name.split_once('.')?;
        let identifier = |text: &str| {
            !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        };
        if !identifier(suite) || !identifier(case) {
            return None;
        }
        let macro_name = self
            .source(test.path)
            .get(test.line.saturating_sub(1))
            .and_then(|line| line.trim_start().split_once('('))
            .map(|(name, _)| name.trim().to_string())?;
        let pattern = match macro_name.as_str() {
            "TEST" | "TEST_F" => format!("^{suite}\\.{case}$"),
            "TEST_P" => format!("/{suite}\\.{case}/"),
            _ => return None,
        };
        self.nearest(parent_dir(test.path), |folder| {
            folder.cmake_gtest.then_some(())
        })?;
        let (dir, build) = self.nearest(parent_dir(test.path), |folder| {
            folder.ctest_build_dir.clone()
        })?;
        Some(in_folder(
            &dir,
            format!(
                "ctest --test-dir {} -R {}",
                shell_word(&build),
                single_quoted(&pattern)
            ),
        ))
    }

    /// PHPUnit's `--filter` on `Class::name` under the nearest `composer.json` or PHPUnit
    /// configuration, with `vendor/bin/phpunit` when Composer installs it.
    fn phpunit(&mut self, test: &TestSpec) -> Option<String> {
        let class = test.classes.last()?;
        let (dir, php) = self.nearest(parent_dir(test.path), |folder| folder.php.clone())?;
        if !php.phpunit {
            return None;
        }
        let tool = if php.composer {
            "vendor/bin/phpunit"
        } else {
            "phpunit"
        };
        let filter = format!("{}::{}\\b", class.name, test.name);
        Some(in_folder(
            &dir,
            format!(
                "{tool} --filter {} {}",
                single_quoted(&filter),
                shell_word(relative(&dir, test.path))
            ),
        ))
    }

    /// `mix test path:line` under the nearest `mix.exs`.
    fn mix(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, ()) = self.nearest(parent_dir(test.path), |folder| folder.mix.then_some(()))?;
        Some(in_folder(
            &dir,
            format!(
                "mix test {}:{}",
                shell_word(relative(&dir, test.path)),
                test.line
            ),
        ))
    }

    /// Common Test for a case of a `_SUITE` module, else EUnit for a `_test` function or a
    /// `_test_` generator, through `rebar3` under the nearest `rebar.config`.
    fn rebar(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, ()) = self.nearest(parent_dir(test.path), |folder| folder.rebar.then_some(()))?;
        let path = relative(&dir, test.path);
        let module_path = path.strip_suffix(".erl")?;
        let module = module_path.rsplit('/').next()?;
        let command = if module.ends_with("_SUITE") {
            format!(
                "rebar3 ct --suite={} --case={}",
                shell_word(module_path),
                shell_word(test.name)
            )
        } else if test.name.ends_with("_test_") {
            format!(
                "rebar3 eunit --generator={}",
                shell_word(&format!("{module}:{}", test.name))
            )
        } else if test.name.ends_with("_test") {
            format!(
                "rebar3 eunit --test={}",
                shell_word(&format!("{module}:{}", test.name))
            )
        } else {
            return None;
        };
        Some(in_folder(&dir, command))
    }

    /// `busted --filter` with the name as a Lua pattern anchored at the end of the full name,
    /// under the nearest `.busted` file or a rockspec that names busted.
    fn busted(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, configured) = self.nearest(parent_dir(test.path), |folder| folder.busted)?;
        if !configured || test.name.contains('\\') {
            return None;
        }
        let mut pattern = String::new();
        for c in test.name.chars() {
            if "^$()%.[]*+-?".contains(c) {
                pattern.push('%');
            }
            pattern.push(c);
        }
        pattern.push('$');
        Some(in_folder(
            &dir,
            format!(
                "busted {} --filter={}",
                shell_word(relative(&dir, test.path)),
                single_quoted(&pattern)
            ),
        ))
    }

    /// `testthat::test_file` with `desc` under the nearest R `DESCRIPTION` that names testthat,
    /// loading the package from source.
    fn testthat(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, package) =
            self.nearest(parent_dir(test.path), |folder| folder.testthat.clone())?;
        let package = package?;
        if test.name.contains('\\') {
            return None;
        }
        let code = format!(
            "testthat::test_file({}, desc = {}, package = {}, load_package = \"source\")",
            r_string(relative(&dir, test.path)),
            r_string(test.name),
            r_string(&package)
        );
        Some(in_folder(
            &dir,
            format!("Rscript -e {}", single_quoted(&code)),
        ))
    }

    /// A test method under the nearest `project.godot`: gdUnit4's `runtest.sh` for the suite of a
    /// file that extends `GdUnitTestSuite` when the project has the gdUnit4 add-on, else GUT's
    /// command line for the method when the project has the GUT add-on. gdUnit4 selects suites,
    /// not test cases, so its command runs the whole file.
    fn godot(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, godot) = self.nearest(parent_dir(test.path), |folder| folder.godot.clone())?;
        if test.kind != "method" {
            return None;
        }
        let resource = format!("res://{}", relative(&dir, test.path));
        let gdunit_suite = self.source(test.path).iter().any(|line| {
            line.trim_start().starts_with("extends") && line.contains("GdUnitTestSuite")
        });
        let command = if gdunit_suite {
            if !godot.gdunit4 {
                return None;
            }
            format!("./addons/gdUnit4/runtest.sh -a {}", shell_word(&resource))
        } else if godot.gut {
            format!(
                "godot --headless -s addons/gut/gut_cmdln.gd {} {} -gexit",
                shell_word(&format!("-gtest={resource}")),
                shell_word(&format!("-gunit_test_name={}", test.name))
            )
        } else {
            return None;
        };
        Some(in_folder(&dir, command))
    }

    /// A C test. A Unity test of a Ceedling project runs through `ceedling test:file
    /// --test-case=name`. Another C test runs through CTest: the `add_test` that runs the one
    /// executable whose sources hold the file, in the one configured build folder. CTest runs the
    /// whole executable, so a CMocka test sets `CMOCKA_TEST_FILTER` and a Check test sets
    /// `CK_RUN_CASE` to the test case that registers it.
    fn c_test(&mut self, test: &TestSpec) -> Option<String> {
        let includes = |source: &[String], header: &str| {
            source.iter().any(|line| {
                let line = line.trim_start();
                line.starts_with('#') && line.contains("include") && line.contains(header)
            })
        };
        let source = self.source(test.path);
        let unity = includes(source, "unity.h");
        let cmocka = includes(source, "cmocka.h");
        let check_case = includes(source, "check.h")
            .then(|| check_test_case(source, test.name))
            .flatten();
        let file = test.path.rsplit('/').next()?;
        if unity
            && let Some((dir, prefix)) = self.nearest(parent_dir(test.path), |folder| {
                folder.ceedling_prefix.clone()
            })
        {
            if !file.starts_with(&prefix) {
                return None;
            }
            return Some(in_folder(
                &dir,
                format!(
                    "ceedling test:{} --test-case={}",
                    shell_word(file),
                    shell_word(test.name)
                ),
            ));
        }
        let name = self.ctest_name(test.path)?;
        let (dir, build) = self.nearest(parent_dir(test.path), |folder| {
            folder.ctest_build_dir.clone()
        })?;
        let filter = if cmocka {
            format!("CMOCKA_TEST_FILTER={} ", shell_word(test.name))
        } else if let Some(case) = check_case {
            format!("CK_RUN_CASE={} ", shell_word(&case))
        } else {
            String::new()
        };
        Some(in_folder(
            &dir,
            format!(
                "{filter}ctest --test-dir {} -R {}",
                shell_word(&build),
                single_quoted(&format!("^{}$", cmake_regex_escaped(&name)))
            ),
        ))
    }

    /// The name of the one CTest test that runs the one executable whose sources hold `path`. A
    /// `CMakeLists.txt` at or above the file names the executable, and that file or one above it
    /// enables testing and adds the test.
    fn ctest_name(&mut self, path: &str) -> Option<String> {
        let mut dir = parent_dir(path);
        let (cmake_dir, target) = loop {
            let found = self.folder(dir).cmake_tests.as_ref().map(|cmake| {
                cmake
                    .sources
                    .iter()
                    .filter(|(_, source)| cmake_source_path(dir, source).as_deref() == Some(path))
                    .map(|(target, _)| target.clone())
                    .collect::<Vec<_>>()
            });
            match found.as_deref() {
                Some([target]) => break (dir.to_string(), target.clone()),
                Some([_, _, ..]) => return None,
                _ if dir.is_empty() => return None,
                _ => dir = parent_dir(dir),
            }
        };
        let mut dir = cmake_dir.as_str();
        let mut enables_testing = false;
        let mut names = Vec::new();
        loop {
            if let Some(cmake) = &self.folder(dir).cmake_tests {
                enables_testing |= cmake.enables_testing;
                names.extend(
                    cmake
                        .tests
                        .iter()
                        .filter(|(_, program)| cmake_test_target(program) == target)
                        .map(|(name, _)| name.clone()),
                );
            }
            if dir.is_empty() {
                break;
            }
            dir = parent_dir(dir);
        }
        match names.as_slice() {
            [name] if enables_testing => Some(name.clone()),
            _ => None,
        }
    }

    /// `zig test path --test-filter` under the nearest `build.zig`, or `zig build test
    /// -Dtest-filter=…` when that file declares the option. The filter is `test.name` for
    /// `test "name"` and `decltest.name` for a doctest `test name`, as the test runner names them.
    /// A test with no name runs under every filter, so it has no command.
    fn zig(&mut self, test: &TestSpec) -> Option<String> {
        let line = self
            .source(test.path)
            .get(test.line.saturating_sub(1))?
            .trim_start();
        let declared = line.strip_prefix("test")?;
        let declared = declared.strip_prefix(char::is_whitespace)?.trim_start();
        let filter = if declared.starts_with('"') {
            if test.name.contains('\\') {
                return None;
            }
            format!("test.{}", test.name)
        } else if declared.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            format!("decltest.{}", test.name)
        } else {
            return None;
        };
        let (dir, build) = self.nearest(parent_dir(test.path), |folder| folder.zig)?;
        let command = if build.filter_option {
            format!("zig build test -Dtest-filter={}", single_quoted(&filter))
        } else {
            format!(
                "zig test {} --test-filter {}",
                shell_word(relative(&dir, test.path)),
                single_quoted(&filter)
            )
        };
        Some(in_folder(&dir, command))
    }

    /// `qmltestrunner -input path 'Case::name'` for a test function of a Qt Quick Test file named
    /// `tst_*.qml`, where `Case` is the `name` of its `TestCase`. A `TestCase` without a literal
    /// name runs no function that the command line selects, so its tests have no command.
    fn qml(&mut self, test: &TestSpec) -> Option<String> {
        let file = test.path.rsplit('/').next()?;
        if !file.starts_with("tst_") || !file.ends_with(".qml") {
            return None;
        }
        let case = qml_test_case_name(self.source(test.path), test.line)?;
        if case.contains("::") {
            return None;
        }
        Some(format!(
            "qmltestrunner -platform offscreen -input {} {}",
            shell_word(test.path),
            shell_word(&format!("{case}::{}", test.name))
        ))
    }
}

/// The name of the Check test case that registers `test` with `tcase_add_test(tc, test)` or
/// another `tcase_add_*` call, where `tc = tcase_create("name")`. `None` unless exactly one test
/// case name is found.
fn check_test_case(source: &[String], test: &str) -> Option<String> {
    let cases: Vec<&str> = source
        .iter()
        .filter_map(|line| {
            let call = &line[line.find("tcase_add_")?..];
            let inner = call.split_once('(')?.1.split_once(')')?.0;
            let mut arguments = inner.split(',').map(str::trim);
            let variable = arguments.next()?;
            (arguments.next() == Some(test)).then_some(variable)
        })
        .filter_map(|variable| {
            source.iter().find_map(|line| {
                let (before, after) = line.split_once("tcase_create(")?;
                let assigned = before.trim_end().strip_suffix('=')?.trim_end();
                let name = assigned
                    .rsplit(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .next()?;
                (name == variable)
                    .then(|| after.trim_start().strip_prefix('"')?.split_once('"'))
                    .flatten()
                    .map(|(case, _)| case)
            })
        })
        .collect();
    match cases.as_slice() {
        [case] => Some(case.to_string()),
        [first, rest @ ..] if rest.iter().all(|case| case == first) => Some(first.to_string()),
        _ => None,
    }
}

/// The `name: "…"` of the innermost `TestCase` object that holds line `line`. Braces inside
/// strings and `//` comments do not count.
fn qml_test_case_name(source: &[String], line: usize) -> Option<String> {
    let mut objects: Vec<(bool, Option<String>)> = Vec::new();
    for text in source.iter().take(line.saturating_sub(1)) {
        if let Some(value) = text.trim_start().strip_prefix("name:")
            && let Some((is_case, name)) = objects.last_mut()
            && *is_case
        {
            *name = value
                .trim_start()
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .map(|(literal, _)| literal)
                .filter(|literal| !literal.contains('\\'))
                .map(str::to_string);
        }
        let code = qml_code(text);
        let mut word_before = String::new();
        let mut word = String::new();
        for c in code.chars() {
            match c {
                '{' => {
                    let label = if word.is_empty() { &word_before } else { &word };
                    let is_case = label.rsplit('.').next() == Some("TestCase");
                    objects.push((is_case, None));
                    word.clear();
                    word_before.clear();
                }
                '}' => {
                    objects.pop();
                    word.clear();
                    word_before.clear();
                }
                c if c.is_ascii_alphanumeric() || c == '_' || c == '.' => word.push(c),
                c if c.is_whitespace() => {
                    if !word.is_empty() {
                        word_before = std::mem::take(&mut word);
                    }
                }
                _ => {
                    word.clear();
                    word_before.clear();
                }
            }
        }
    }
    objects
        .into_iter()
        .rev()
        .find(|(is_case, _)| *is_case)
        .and_then(|(_, name)| name)
}

/// A line of QML without its `//` comment and with each string literal emptied.
fn qml_code(line: &str) -> String {
    let mut code = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' | '\'' | '`' => {
                code.push(c);
                while let Some(inner) = chars.next() {
                    if inner == '\\' {
                        chars.next();
                    } else if inner == c {
                        break;
                    }
                }
                code.push(c);
            }
            '/' if chars.peek() == Some(&'/') => break,
            c => code.push(c),
        }
    }
    code
}

/// `dotnet test --filter` on `Owner.name`: the innermost class, or for F# and VB.NET the module
/// that holds the test. F# writes a name with spaces between double backticks.
fn dotnet(test: &TestSpec) -> Option<String> {
    let owner = match test.classes.last() {
        Some(class) => class.name,
        None if test.language != "csharp" => test.module?,
        None => return None,
    };
    let name = test
        .name
        .strip_prefix("``")
        .and_then(|name| name.strip_suffix("``"))
        .unwrap_or(test.name);
    let filter = double_quoted(&format!("{owner}.{name}"));
    Some(format!(
        "dotnet test --filter \"FullyQualifiedName~{filter}\""
    ))
}

/// Pester's `-FullNameFilter` for an `It` block of a `.Tests.ps1` file, as a wildcard over the
/// full name `Describe.Context.It`.
fn pester(test: &TestSpec) -> Option<String> {
    if !test.path.to_ascii_lowercase().ends_with(".tests.ps1") {
        return None;
    }
    let mut filter = String::from("*.");
    for c in test.name.chars() {
        if matches!(c, '[' | ']' | '*' | '?' | '`') {
            filter.push('`');
        }
        filter.push(c);
    }
    let script = format!(
        "Invoke-Pester -Path {} -FullNameFilter {}",
        powershell_string(test.path),
        powershell_string(&filter)
    );
    Some(format!(
        "pwsh -NoProfile -Command \"{}\"",
        double_quoted(&script)
    ))
}

/// `bats --filter` with the test name as an anchored regular expression.
fn bats(test: &TestSpec) -> Option<String> {
    if !test.path.ends_with(".bats") || test.name.contains('\\') {
        return None;
    }
    let mut filter = String::from("^");
    regex_escaped_into(&mut filter, test.name);
    filter.push('$');
    Some(format!(
        "bats {} --filter {}",
        shell_word(test.path),
        single_quoted(&filter)
    ))
}

fn regex_escaped_into(out: &mut String, text: &str) {
    for c in text.chars() {
        if "\\^$.*+?()[]{}|/".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
}

/// `text` as an R double-quoted string.
fn r_string(text: &str) -> String {
    format!("\"{}\"", backslashed(text, |c| matches!(c, '"' | '\\')))
}

/// `text` as a PowerShell single-quoted string.
fn powershell_string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_tempdir;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = safe_tempdir();
        for (path, content) in files {
            let full = temp.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        temp
    }

    fn command(
        project: &tempfile::TempDir,
        language: &str,
        path: &str,
        name: &str,
        class: Option<&str>,
    ) -> Option<String> {
        nested(project, language, path, name, class.as_slice())
    }

    fn script(files: &[(&str, &str)], path: &str, name: &str) -> Option<String> {
        command(&project(files), "typescript", path, name, None)
    }

    fn nested(
        project: &tempfile::TempDir,
        language: &str,
        path: &str,
        name: &str,
        classes: &[&str],
    ) -> Option<String> {
        let classes: Vec<TestClass> = classes
            .iter()
            .map(|name| TestClass {
                name,
                test_case: false,
            })
            .collect();
        CommandFinder::new(project.path()).command(&TestSpec {
            language,
            kind: "function",
            path,
            line: 1,
            name,
            classes: &classes,
            module: None,
            groups: &[],
        })
    }

    #[test]
    fn python_tests_run_through_pytest_with_the_class_only_for_a_member_of_one() {
        let empty = project(&[]);

        assert_eq!(
            command(&empty, "python", "tests/test_a.py", "test_one", None).as_deref(),
            Some("pytest tests/test_a.py::test_one")
        );
        assert_eq!(
            command(
                &empty,
                "python",
                "tests/test_a.py",
                "test_one",
                Some("TestA")
            )
            .as_deref(),
            Some("pytest tests/test_a.py::TestA::test_one")
        );
    }

    #[test]
    fn a_rust_test_names_the_package_of_the_nearest_cargo_toml() {
        let crate_dir = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/a\"]\n\n[package]\nname = \"outer\"\n",
            ),
            (
                "crates/a/Cargo.toml",
                "[dependencies]\nname = \"not-this\"\n\n[package]\nversion = \"0.1.0\"\nname = \"inner-crate\" # the crate\n\n[lib]\nname = \"also_not\"\n",
            ),
        ]);

        let run = |path: &str| command(&crate_dir, "rust", path, "case", None);

        assert_eq!(
            run("crates/a/src/lib.rs").as_deref(),
            Some("cargo test -p inner-crate case")
        );
        assert_eq!(
            run("crates/a/tests/it.rs").as_deref(),
            Some("cargo test -p inner-crate --test it case")
        );
        assert_eq!(
            run("crates/a/tests/sub/it.rs").as_deref(),
            Some("cargo test -p inner-crate case")
        );
        assert_eq!(
            run("src/main.rs").as_deref(),
            Some("cargo test -p outer case")
        );
    }

    #[test]
    fn a_rust_test_without_a_package_name_runs_by_name_alone() {
        let none = project(&[]);
        let workspace_only = project(&[("Cargo.toml", "[workspace]\nmembers = []\n")]);

        assert_eq!(
            command(&none, "rust", "tests/it.rs", "case", None).as_deref(),
            Some("cargo test case")
        );
        assert_eq!(
            command(&workspace_only, "rust", "src/lib.rs", "case", None).as_deref(),
            Some("cargo test case")
        );
    }

    #[test]
    fn a_go_test_names_its_folder_relative_to_the_nearest_go_mod() {
        let module = project(&[
            ("go.mod", "module example.com/m\n"),
            ("sub/go.mod", "module example.com/m/sub\n"),
        ]);

        let run = |path: &str| command(&module, "go", path, "TestRun", None);

        assert_eq!(
            run("pkg/run_test.go").as_deref(),
            Some("go test ./pkg -run '^TestRun$'")
        );
        assert_eq!(
            run("run_test.go").as_deref(),
            Some("go test ./. -run '^TestRun$'")
        );
        assert_eq!(
            run("sub/deep/pkg/run_test.go").as_deref(),
            Some("cd sub && go test ./deep/pkg -run '^TestRun$'")
        );
        assert_eq!(
            run("sub/run_test.go").as_deref(),
            Some("cd sub && go test ./. -run '^TestRun$'")
        );
    }

    #[test]
    fn a_script_test_picks_the_runner_the_nearest_package_json_names() {
        let run = |body: &str| {
            script(
                &[("package.json", body)],
                "src/a.test.ts",
                "adds \"two\" numbers",
            )
        };

        assert_eq!(
            run(r#"{"devDependencies":{"vitest":"^1"}}"#).as_deref(),
            Some(r#"npx vitest run src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"dependencies":{"jest":"^29"}}"#).as_deref(),
            Some(r#"npx jest --runTestsByPath src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"mocha":"^10"}}"#).as_deref(),
            Some(r#"npx mocha src/a.test.ts --fgrep "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"node --test"}}"#).as_deref(),
            Some(r#"node --test-name-pattern "^adds \"two\" numbers\$" src/a.test.ts"#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"cross-env CI=1 jest --runInBand"}}"#).as_deref(),
            Some(r#"npx jest --runTestsByPath src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(run(r#"{"scripts":{"test":"tap"}}"#), None);
        assert_eq!(run("not json"), None);
    }

    #[test]
    fn a_script_test_uses_the_nearest_package_json_only() {
        let files = [
            ("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#),
            ("pkg/package.json", r#"{"type":"module"}"#),
        ];

        assert_eq!(script(&files, "pkg/a.test.ts", "case"), None);
        assert_eq!(
            script(&files, "other/a.test.ts", "case").as_deref(),
            Some(r#"npx vitest run other/a.test.ts -t "case""#)
        );
        assert_eq!(script(&[], "a.test.ts", "case"), None);
    }

    #[test]
    fn a_script_name_escapes_the_characters_a_double_quoted_shell_word_expands() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", r#"costs $5 `now` a\\b"#).as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "costs \$5 \`now\` a\\b""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_escapes_the_characters_of_a_regular_expression() {
        let name = "should not render evil javascript links in anchor href (prevent XSS)";
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", name).as_deref(),
            Some(
                r#"npx vitest run a.test.ts -t "should not render evil javascript links in anchor href \\(prevent XSS\\)""#
            )
        );
        assert_eq!(
            script(&jest, "a.test.ts", "parses v1.2 (beta)").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t "parses v1\\.2 \\(beta\\)""#)
        );
    }

    #[test]
    fn a_vitest_name_escapes_every_regular_expression_character_before_the_shell_quoting() {
        let files = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", r#"\\ ^ $ . * + ? ( ) [ ] { } | "x""#).as_deref(),
            Some(
                r#"npx vitest run a.test.ts -t "\\\\ \\^ \\\$ \\. \\* \\+ \\? \\( \\) \\[ \\] \\{ \\} \\| \"x\"""#
            )
        );
    }

    #[test]
    fn a_mocha_name_is_a_plain_substring_so_it_keeps_its_regular_expression_characters() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "parses v1.2 (beta)").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "parses v1.2 (beta)""#)
        );
    }

    #[test]
    fn a_script_name_decodes_its_source_escapes_before_any_escaping() {
        let run = |dependency: &str, name: &str| {
            script(
                &[(
                    "package.json",
                    &format!(r#"{{"devDependencies":{{"{dependency}":"^1"}}}}"#),
                )],
                "a.test.js",
                name,
            )
        };

        assert_eq!(
            run("vitest", r"doesn\'t throw on empty input").as_deref(),
            Some(r#"npx vitest run a.test.js -t "doesn't throw on empty input""#)
        );
        assert_eq!(
            run("jest", r#"says \"hi\" politely"#).as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.js -t "says \"hi\" politely""#)
        );
        assert_eq!(
            run("vitest", r"tick \` and back\\slash").as_deref(),
            Some(r#"npx vitest run a.test.js -t "tick \` and back\\\\slash""#)
        );
        assert_eq!(
            run("vitest", r"two\nlines\tand\rmore").as_deref(),
            Some(r#"npx vitest run a.test.js -t "two\\nlines\\tand\\rmore""#)
        );
        assert_eq!(
            run("mocha", r#"doesn\'t say \"hi\" or back\\slash"#).as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "doesn't say \"hi\" or back\\slash""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_turns_each_format_placeholder_into_a_wildcard() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(
                &vitest,
                "a.test.ts",
                "adds %i + %d = %s, %f %j %o %O %p #%#"
            )
            .as_deref(),
            Some(r#"npx vitest run a.test.ts -t "adds .* \\+ .* = .*, .* .* .* .* .* #.*""#)
        );
        assert_eq!(
            script(
                &jest,
                "a.test.ts",
                "$a is $b.c.d. costs $5 or 100% and 50%x"
            )
            .as_deref(),
            Some(
                r#"npx jest --runTestsByPath a.test.ts -t ".* is .*\\. costs \\\$5 or 100% and 50%x""#
            )
        );
    }

    #[test]
    fn a_mocha_name_keeps_its_format_placeholders() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "adds %i + %s for $name").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "adds %i + %s for \$name""#)
        );
    }

    #[test]
    fn a_mocha_name_keeps_a_double_percent_as_written() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "pct 100%% sure").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "pct 100%% sure""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_matches_a_double_percent_as_one_or_two_percent_signs() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", "pct 100%% sure %s").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "pct 100%%? sure .*""#)
        );
        assert_eq!(
            script(&jest, "a.test.ts", "%%%s%%").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t "%%?.*%%?""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_turns_each_template_hole_into_a_wildcard() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", "handles ${kind} kind").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "handles .* kind""#)
        );
        assert_eq!(
            script(&jest, "a.test.ts", "${a} and ${b.c}! (${ {x: 1}.x })").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t ".* and .*! \\(.*\\)""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", "costs ${5").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "costs \\\$\\{5""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", r"costs \${5} total").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "costs \\\$\\{5\\} total""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", "$a and ${b}$c").as_deref(),
            Some(r#"npx vitest run a.test.ts -t ".* and .*.*""#)
        );
    }

    #[test]
    fn a_mocha_name_with_a_template_hole_uses_its_longest_literal_part() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&files, "a.test.ts", name);

        assert_eq!(
            run("handles ${kind} kind").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "handles ""#)
        );
        assert_eq!(
            run("a ${x} and a longer ${ {y: 1}.y } tail").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep " and a longer ""#)
        );
        assert_eq!(
            run("ab${x}cd").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "ab""#)
        );
        assert_eq!(
            run(r"costs \${5} total").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "costs \${5} total""#)
        );
    }

    #[test]
    fn a_mocha_name_with_no_literal_part_has_no_name_filter() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&files, "a.test.ts", name);

        assert_eq!(run("${kind}").as_deref(), Some("npx mocha a.test.ts"));
        assert_eq!(run("${a} ${b}").as_deref(), Some("npx mocha a.test.ts"));
    }

    #[test]
    fn a_script_name_decodes_hex_unicode_and_octal_escapes() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"caf\xe9 é \u{1F600} 😀").as_deref(),
            Some("npx vitest run a.test.js -t \"café é 😀 😀\"")
        );
        assert_eq!(
            script(&mocha, "a.test.js", r"\101\60\8 \x41B \\x41").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "A08 AB \\x41""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"a\bb\fc\vd").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\x08b\\x0cc\\x0bd""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"bad \xZ1 \u12 \u{110000} \u{}").as_deref(),
            Some(r#"npx vitest run a.test.js -t "bad xZ1 u12 u\\{110000\\} u\\{\\}""#)
        );
    }

    #[test]
    fn a_character_a_command_cannot_carry_splits_a_script_name_like_a_template_hole() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"a\0b\x00c\ud83dd").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a.*b.*c.*d""#)
        );
        assert_eq!(
            script(&mocha, "a.test.js", r"a\0bcd\ud83de").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "bcd""#)
        );
    }

    #[test]
    fn a_backslash_before_a_multibyte_character_or_at_the_end_of_a_name_keeps_the_hole_scan_in_step()
     {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"caf\é ${x} é${y}").as_deref(),
            Some(r#"npx vitest run a.test.js -t "café .* é.*""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"end\").as_deref(),
            Some(r#"npx vitest run a.test.js -t "end\\\\""#)
        );
    }

    #[test]
    fn a_path_with_a_space_or_shell_character_is_single_quoted_in_every_command() {
        let scripts =
            |dependency: &str| format!(r#"{{"devDependencies":{{"{dependency}":"^1"}}}}"#);
        let tricky = "it's $x/a b.test.ts";
        let quoted = r#"'it'\''s $x/a b.test.ts'"#;
        let run =
            |dependency: &str| script(&[("package.json", &scripts(dependency))], tricky, "case");

        assert_eq!(
            run("vitest").as_deref(),
            Some(format!(r#"npx vitest run {quoted} -t "case""#).as_str())
        );
        assert_eq!(
            run("jest").as_deref(),
            Some(format!(r#"npx jest --runTestsByPath {quoted} -t "case""#).as_str())
        );
        assert_eq!(
            run("mocha").as_deref(),
            Some(format!(r#"npx mocha {quoted} --fgrep "case""#).as_str())
        );
        assert_eq!(
            script(
                &[("package.json", r#"{"scripts":{"test":"node --test"}}"#)],
                tricky,
                "case"
            )
            .as_deref(),
            Some(format!(r#"node --test-name-pattern "^case\$" {quoted}"#).as_str())
        );
    }

    #[test]
    fn a_path_with_a_space_is_single_quoted_for_pytest_cargo_and_go() {
        let empty = project(&[]);
        let cargo = project(&[("Cargo.toml", "[package]\nname = \"p\"\n")]);
        let module = project(&[("go.mod", "module m\n")]);

        assert_eq!(
            command(&empty, "python", "tests/a b.py", "test_one", Some("TestA")).as_deref(),
            Some("pytest 'tests/a b.py'::TestA::test_one")
        );
        assert_eq!(
            command(&empty, "python", "tests/a_b.py", "test_one", None).as_deref(),
            Some("pytest tests/a_b.py::test_one")
        );
        assert_eq!(
            command(&cargo, "rust", "tests/my test.rs", "case", None).as_deref(),
            Some("cargo test -p p --test 'my test' case")
        );
        assert_eq!(
            command(&module, "go", "my pkg/run_test.go", "TestRun", None).as_deref(),
            Some("go test './my pkg' -run '^TestRun$'")
        );
    }

    #[test]
    fn a_jest_command_names_its_file_by_path_so_no_character_is_a_regular_expression() {
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        for (path, word) in [
            ("test/(grp)/c.test.js", "'test/(grp)/c.test.js'"),
            ("test/[id]/c.test.js", "'test/[id]/c.test.js'"),
            ("test/a+b/c.test.js", "'test/a+b/c.test.js'"),
            ("test/d$x/c.test.js", "'test/d$x/c.test.js'"),
            ("test/a.test.js", "test/a.test.js"),
        ] {
            assert_eq!(
                script(&jest, path, "case").as_deref(),
                Some(format!(r#"npx jest --runTestsByPath {word} -t "case""#).as_str()),
                "{path}"
            );
        }
    }

    #[test]
    fn a_node_test_command_runs_the_file_itself_so_no_character_is_a_glob() {
        let files = [("package.json", r#"{"scripts":{"test":"node --test"}}"#)];

        for (path, word) in [
            ("test/[id]/a.test.js", "'test/[id]/a.test.js'"),
            ("test/+(x)/a.test.js", "'test/+(x)/a.test.js'"),
            ("test/!(x)/a.test.js", "'test/!(x)/a.test.js'"),
            ("test/{a,b}/a.test.js", "'test/{a,b}/a.test.js'"),
            ("test/a.test.js", "test/a.test.js"),
        ] {
            assert_eq!(
                script(&files, path, "case").as_deref(),
                Some(format!(r#"node --test-name-pattern "^case\$" {word}"#).as_str()),
                "{path}"
            );
        }
    }

    #[test]
    fn a_python_test_in_a_nested_class_names_every_enclosing_class() {
        let empty = project(&[]);

        assert_eq!(
            nested(
                &empty,
                "python",
                "tests/test_n.py",
                "test_deep",
                &["TestOuter", "TestInner"]
            )
            .as_deref(),
            Some("pytest tests/test_n.py::TestOuter::TestInner::test_deep")
        );
        assert_eq!(
            nested(
                &empty,
                "python",
                "tests/test_n.py",
                "test_top",
                &["TestOuter"]
            )
            .as_deref(),
            Some("pytest tests/test_n.py::TestOuter::test_top")
        );
    }

    #[test]
    fn a_java_test_in_a_nested_class_names_the_binary_class_name_as_one_shell_word() {
        let maven = project(&[("pom.xml", "<project/>")]);
        let gradle = project(&[("gradlew", ""), ("build.gradle", "")]);
        let path = "src/test/java/OuterTest.java";
        let classes = ["OuterTest", "InnerTest", "DeepTest"];

        assert_eq!(
            nested(&maven, "java", path, "checks", &classes).as_deref(),
            Some(r#"mvn test -Dtest='OuterTest$InnerTest$DeepTest'#checks"#)
        );
        assert_eq!(
            nested(&gradle, "java", path, "checks", &classes).as_deref(),
            Some(r#"./gradlew test --tests 'OuterTest$InnerTest$DeepTest.checks'"#)
        );
        assert_eq!(
            nested(&maven, "java", path, "checks", &["OuterTest"]).as_deref(),
            Some("mvn test -Dtest=OuterTest#checks")
        );
    }

    #[test]
    fn a_java_or_csharp_name_with_a_shell_character_stays_one_shell_word() {
        let maven = project(&[("pom.xml", "<project/>")]);
        let gradle = project(&[("build.gradle", "")]);
        let path = "src/test/java/FooTest.java";

        assert_eq!(
            nested(&maven, "java", path, "a$b `c` it's", &["Foo$1"]).as_deref(),
            Some(r#"mvn test -Dtest='Foo$1'#'a$b `c` it'\''s'"#)
        );
        assert_eq!(
            nested(&gradle, "java", path, "a$b it's", &["Foo$1"]).as_deref(),
            Some(r#"gradle test --tests 'Foo$1.a$b it'\''s'"#)
        );
        assert_eq!(
            nested(&maven, "csharp", "T/FooTests.cs", "C\"D$`e`\\f", &["A$B"]).as_deref(),
            Some(r#"dotnet test --filter "FullyQualifiedName~A\$B.C\"D\$\`e\`\\f""#)
        );
    }

    #[test]
    fn a_csharp_test_in_a_nested_class_names_the_innermost_class() {
        let empty = project(&[]);

        assert_eq!(
            nested(
                &empty,
                "csharp",
                "T/FooTests.cs",
                "Checks",
                &["FooTests", "Inner"]
            )
            .as_deref(),
            Some("dotnet test --filter \"FullyQualifiedName~Inner.Checks\"")
        );
    }

    #[test]
    fn a_backslash_before_a_line_break_in_a_script_name_drops_both() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        for name in [
            "part one \\\npart two",
            "part one \\\r\npart two",
            "part one \\\rpart two",
            "part one \\\u{2028}part two",
            "part one \\\u{2029}part two",
        ] {
            assert_eq!(
                script(&vitest, "a.test.js", name).as_deref(),
                Some(r#"npx vitest run a.test.js -t "part one part two""#),
                "{name:?}"
            );
            assert_eq!(
                script(&mocha, "a.test.js", name).as_deref(),
                Some(r#"npx mocha a.test.js --fgrep "part one part two""#),
                "{name:?}"
            );
        }
        assert_eq!(
            script(&vitest, "a.test.js", "a\\\\\nb").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\\\\\nb""#)
        );
    }

    #[test]
    fn a_string_or_template_inside_a_template_hole_does_not_end_the_hole() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let run = |name: &str| script(&vitest, "a.test.js", name);

        for name in [
            r#"literal ${"}"} inside hole"#,
            r"literal ${'}'} inside hole",
            "literal ${`}`} inside hole",
            r#"literal ${"\"}"} inside hole"#,
            r#"literal ${`x ${"}"} y`} inside hole"#,
            r"literal ${f('}', {a: 1}.a)} inside hole",
        ] {
            assert_eq!(
                run(name).as_deref(),
                Some(r#"npx vitest run a.test.js -t "literal .* inside hole""#),
                "{name}"
            );
        }
        assert_eq!(
            run(r#"a ${"} b"#).as_deref(),
            Some(r#"npx vitest run a.test.js -t "a \\\$\\{\"\\} b""#)
        );
    }

    #[test]
    fn a_mocha_name_cuts_at_a_character_that_would_split_the_command_line() {
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&mocha, "a.test.js", name);

        assert_eq!(
            run(r"multi\nline name").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "line name""#)
        );
        assert_eq!(
            run(r"carriage\rreturn").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "carriage""#)
        );
        assert_eq!(
            run(r"backspace \b and formfeed \f").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep " and formfeed ""#)
        );
        assert_eq!(
            run("real\nnewline").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "newline""#)
        );
        assert_eq!(
            run("ab\u{2028}cdef\u{85}gh").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "cdef""#)
        );
        assert_eq!(
            run(r"a\nb\fc").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "a""#)
        );
        assert_eq!(run(r"\n\f").as_deref(), Some("npx mocha a.test.js"));
        assert_eq!(
            run(r"tab\there").as_deref(),
            Some("npx mocha a.test.js --fgrep \"tab\there\"")
        );
    }

    #[test]
    fn a_vitest_or_jest_name_writes_a_unicode_line_separator_as_an_escape() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", "a\u{2028}b\u{2029}c").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\u2028b\\u2029c""#)
        );
    }

    #[test]
    fn a_go_test_below_the_root_changes_into_the_folder_of_its_go_mod() {
        let module = project(&[
            ("svc/go.mod", "module example.com/svc\n"),
            ("my svc/go.mod", "module example.com/mine\n"),
            ("a/b/go.mod", "module example.com/ab\n"),
        ]);

        let run = |path: &str| command(&module, "go", path, "TestAdd", None);

        assert_eq!(
            run("svc/pkg/m_test.go").as_deref(),
            Some("cd svc && go test ./pkg -run '^TestAdd$'")
        );
        assert_eq!(
            run("svc/m_test.go").as_deref(),
            Some("cd svc && go test ./. -run '^TestAdd$'")
        );
        assert_eq!(
            run("my svc/pkg/m_test.go").as_deref(),
            Some("cd 'my svc' && go test ./pkg -run '^TestAdd$'")
        );
        assert_eq!(
            run("a/b/c/m_test.go").as_deref(),
            Some("cd a/b && go test ./c -run '^TestAdd$'")
        );
    }

    #[test]
    fn a_script_test_below_the_root_changes_into_the_folder_of_its_package_json() {
        let run = |manifest: &str, dir: &str| {
            script(
                &[(&format!("{dir}/package.json"), manifest)],
                &format!("{dir}/test/x.test.js"),
                "case",
            )
        };

        assert_eq!(
            run(r#"{"devDependencies":{"vitest":"^1"}}"#, "packages/a").as_deref(),
            Some(r#"cd packages/a && npx vitest run test/x.test.js -t "case""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"jest":"^29"}}"#, "packages/a").as_deref(),
            Some(r#"cd packages/a && npx jest --runTestsByPath test/x.test.js -t "case""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"mocha":"^10"}}"#, "web").as_deref(),
            Some(r#"cd web && npx mocha test/x.test.js --fgrep "case""#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"node --test"}}"#, "my pkg").as_deref(),
            Some(r#"cd 'my pkg' && node --test-name-pattern "^case\$" test/x.test.js"#)
        );
    }

    #[test]
    fn a_rust_test_changes_into_its_crate_unless_the_root_manifest_is_a_workspace() {
        let member = "[package]\nname = \"inner\"\n";
        let no_root = project(&[("crates/n/Cargo.toml", member)]);
        let plain_root = project(&[
            ("Cargo.toml", "[package]\nname = \"outer\"\n"),
            ("crates/n/Cargo.toml", member),
        ]);
        let workspace_root = project(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"crates/n\"]\n"),
            ("crates/n/Cargo.toml", member),
        ]);
        let unnamed = project(&[("crates/n/Cargo.toml", "[workspace]\nmembers = []\n")]);

        let run =
            |project: &tempfile::TempDir, path: &str| command(project, "rust", path, "case", None);

        assert_eq!(
            run(&no_root, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner case")
        );
        assert_eq!(
            run(&no_root, "crates/n/tests/it.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner --test it case")
        );
        assert_eq!(
            run(&plain_root, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner case")
        );
        assert_eq!(
            run(&plain_root, "src/lib.rs").as_deref(),
            Some("cargo test -p outer case")
        );
        assert_eq!(
            run(&workspace_root, "crates/n/src/lib.rs").as_deref(),
            Some("cargo test -p inner case")
        );
        assert_eq!(
            run(&unnamed, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test case")
        );
    }

    #[test]
    fn a_python_test_below_the_root_runs_from_the_root_by_its_path() {
        let nested_project = project(&[("pkg/pyproject.toml", "[project]\nname = \"p\"\n")]);

        assert_eq!(
            command(
                &nested_project,
                "python",
                "pkg/tests/test_a.py",
                "test_one",
                None
            )
            .as_deref(),
            Some("pytest pkg/tests/test_a.py::test_one")
        );
    }

    #[test]
    fn java_tests_run_through_the_nearest_maven_or_gradle_build() {
        let maven = project(&[("app/pom.xml", "<project/>")]);
        let wrapped = project(&[("gradlew", ""), ("app/build.gradle", "")]);
        let kotlin_dsl = project(&[("gradlew", ""), ("app/build.gradle.kts", "")]);
        let bare = project(&[("app/build.gradle", "")]);
        let both = project(&[("pom.xml", "<project/>"), ("build.gradle", "")]);
        let none = project(&[]);
        let path = "app/src/test/java/FooTest.java";

        let run =
            |project: &tempfile::TempDir| command(project, "java", path, "checks", Some("FooTest"));

        assert_eq!(
            run(&maven).as_deref(),
            Some("cd app && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&wrapped).as_deref(),
            Some("cd app && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&kotlin_dsl).as_deref(),
            Some("cd app && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&bare).as_deref(),
            Some("cd app && gradle test --tests 'FooTest.checks'")
        );
        assert_eq!(
            command(&both, "java", path, "checks", Some("FooTest")).as_deref(),
            Some("mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(run(&none), None);
        assert_eq!(command(&maven, "java", path, "checks", None), None);
    }

    #[test]
    fn csharp_tests_run_through_dotnet_test_with_a_class_filter() {
        let empty = project(&[]);

        assert_eq!(
            command(
                &empty,
                "csharp",
                "T/FooTests.cs",
                "Checks",
                Some("FooTests")
            )
            .as_deref(),
            Some("dotnet test --filter \"FullyQualifiedName~FooTests.Checks\"")
        );
        assert_eq!(
            command(&empty, "csharp", "T/FooTests.cs", "Checks", None),
            None
        );
    }

    #[test]
    fn a_language_without_a_configured_runner_has_no_command() {
        let empty = project(&[]);

        for language in [
            "ruby", "php", "kotlin", "scala", "swift", "dart", "cpp", "c", "elixir", "erlang",
            "lua", "r", "gdscript", "zig", "qml", "vue",
        ] {
            assert_eq!(
                command(&empty, language, "tests/a_test.x", "case", Some("A")),
                None,
                "{language}"
            );
        }
    }

    #[test]
    fn a_broader_command_uses_only_a_configured_package_script() {
        let configured = project(&[("package.json", r#"{"scripts":{"test":"node --test"}}"#)]);
        let unconfigured = project(&[("package.json", r#"{"scripts":{"build":"node"}}"#)]);
        assert_eq!(
            CommandFinder::new(configured.path()).broader_test_command(None),
            Some("npm test".into())
        );
        assert_eq!(
            CommandFinder::new(unconfigured.path()).broader_test_command(None),
            None
        );
    }

    #[test]
    fn a_broader_command_ignores_the_npm_init_placeholder_script() {
        let placeholder = project(&[(
            "package.json",
            r#"{"name":"demo","version":"1.0.0","scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )]);
        let nested_placeholder = project(&[
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            (
                "packages/app/package.json",
                r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
            ),
        ]);
        let real_echo = project(&[(
            "package.json",
            r#"{"scripts":{"test":"echo start && node --test"}}"#,
        )]);
        assert_eq!(
            CommandFinder::new(placeholder.path()).broader_test_command(None),
            None
        );
        assert_eq!(
            CommandFinder::new(placeholder.path()).broader_test_command(Some("src/a.js")),
            None
        );
        assert_eq!(
            CommandFinder::new(nested_placeholder.path())
                .broader_test_command(Some("packages/app/src/a.js")),
            None
        );
        assert_eq!(
            CommandFinder::new(real_echo.path()).broader_test_command(None),
            Some("npm test".into())
        );
    }

    #[test]
    fn a_broader_gradle_command_for_a_subproject_uses_the_nearest_wrapper_above_it() {
        let root_wrapper = project(&[
            ("gradlew", ""),
            ("gradlew.bat", ""),
            ("settings.gradle", "include 'app'\n"),
            ("app/build.gradle", ""),
        ]);
        let not_included = project(&[
            ("gradlew", ""),
            ("settings.gradle", ""),
            ("app/build.gradle", ""),
        ]);
        let deep_wrapper = project(&[
            ("gradlew", ""),
            ("gradlew.bat", ""),
            ("a/b/build.gradle.kts", ""),
        ]);
        let local_wrapper = project(&[
            ("gradlew", ""),
            ("app/gradlew", ""),
            ("app/gradlew.bat", ""),
            ("app/build.gradle", ""),
        ]);
        let bare = project(&[("app/build.gradle", "")]);
        let (root, deep, local) = if cfg!(windows) {
            (
                "gradlew.bat :app:test",
                "cd a/b && ..\\..\\gradlew.bat test",
                "cd app && gradlew.bat test",
            )
        } else {
            (
                "./gradlew :app:test",
                "cd a/b && ../../gradlew test",
                "cd app && ./gradlew test",
            )
        };
        let broader = |project: &tempfile::TempDir, path: &str| {
            CommandFinder::new(project.path()).broader_test_command(Some(path))
        };

        assert_eq!(
            broader(&root_wrapper, "app/src/test/java/FooTest.java").as_deref(),
            Some(root)
        );
        assert_eq!(
            broader(&deep_wrapper, "a/b/src/test/kotlin/FooTest.kt").as_deref(),
            Some(deep)
        );
        assert_eq!(
            broader(&local_wrapper, "app/src/test/java/FooTest.java").as_deref(),
            Some(local)
        );
        assert_eq!(
            broader(&bare, "app/src/test/java/FooTest.java").as_deref(),
            Some("cd app && gradle test")
        );
        assert_eq!(
            broader(&not_included, "app/src/test/java/FooTest.java"),
            None
        );
    }

    #[test]
    fn a_broader_command_uses_the_nearest_configured_folder() {
        let project = project(&[
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            (
                "packages/test suite/package.json",
                r#"{"scripts":{"test":"node --test"}}"#,
            ),
        ]);
        let mut finder = CommandFinder::new(project.path());

        assert_eq!(
            finder.broader_test_command(Some("packages/test suite/test/a.test.js")),
            Some("cd 'packages/test suite' && npm test".into())
        );
    }

    #[test]
    fn a_broader_python_command_requires_pytest_configuration() {
        let configured = project(&[(
            "pyproject.toml",
            "[project]\nname = 'configured'\n[tool.pytest.ini_options]\npython_files = ['test_*.py']\n",
        )]);
        let unconfigured = project(&[("pyproject.toml", "[project]\nname = 'plain'\n")]);

        assert_eq!(
            CommandFinder::new(configured.path()).broader_test_command(Some("tests/test_a.py")),
            Some("pytest".into())
        );
        assert_eq!(
            CommandFinder::new(unconfigured.path()).broader_test_command(Some("tests/test_a.py")),
            None
        );
    }

    #[test]
    fn a_nested_python_project_without_pytest_config_blocks_an_ancestor_config() {
        let project = project(&[
            ("pytest.ini", "[pytest]\n"),
            (
                "packages/python/pyproject.toml",
                "[project]\nname = 'nested'\n",
            ),
        ]);

        assert_eq!(
            CommandFinder::new(project.path())
                .broader_test_command(Some("packages/python/tests/test_api.py")),
            None
        );
    }

    #[test]
    fn python_project_markers_block_ancestor_pytest_without_marking_lint_only_configs() {
        for (folder, marker, content) in [
            (
                "packages/poetry",
                "pyproject.toml",
                "[tool.poetry]\nname = 'poetry-package'\n",
            ),
            (
                "packages/setupcfg",
                "setup.cfg",
                "[metadata]\nname = legacy-package\n",
            ),
            ("packages/setup", "setup.py", ""),
        ] {
            let marker_path = format!("{folder}/{marker}");
            let project = project(&[("pytest.ini", "[pytest]\n"), (&marker_path, content)]);
            let target = format!("{folder}/tests/test_api.py");

            assert_eq!(
                CommandFinder::new(project.path()).broader_test_command(Some(&target)),
                None,
                "{marker_path}"
            );
        }

        let lint_only = project(&[
            ("pytest.ini", "[pytest]\n"),
            (
                "packages/lint/pyproject.toml",
                "[tool.ruff]\nline-length = 88\n",
            ),
        ]);
        assert_eq!(
            CommandFinder::new(lint_only.path())
                .broader_test_command(Some("packages/lint/tests/test_api.py"))
                .as_deref(),
            Some("pytest")
        );

        let local_config = project(&[
            ("pytest.ini", "[pytest]\n"),
            (
                "packages/configured/pyproject.toml",
                "[tool.poetry]\nname = 'configured-package'\n",
            ),
            ("packages/configured/pytest.ini", "[pytest]\n"),
        ]);
        assert_eq!(
            CommandFinder::new(local_config.path())
                .broader_test_command(Some("packages/configured/tests/test_api.py"))
                .as_deref(),
            Some("cd packages/configured && pytest")
        );
    }

    #[test]
    fn broader_dotnet_command_recognizes_solutions_and_test_project_evidence() {
        for marker in ["app.sln", "app.slnx"] {
            let project = project(&[(marker, "")]);
            assert_eq!(
                CommandFinder::new(project.path()).broader_test_command(None),
                Some("dotnet test".into()),
                "{marker}"
            );
        }

        for marker in ["app.csproj", "app.fsproj", "app.vbproj"] {
            for source in [
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
                "<Project><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.0\" /></ItemGroup></Project>",
            ] {
                let project = project(&[(marker, source)]);
                assert_eq!(
                    CommandFinder::new(project.path()).broader_test_command(None),
                    Some("dotnet test".into()),
                    "{marker} {source}"
                );
            }
        }
    }

    #[test]
    fn broader_dotnet_command_does_not_treat_a_production_project_as_test_scope() {
        for source in [
            "<Project />",
            "<Project><PropertyGroup><IsTestProject>false</IsTestProject></PropertyGroup></Project>",
        ] {
            let production_only = project(&[("app.csproj", source)]);
            assert_eq!(
                CommandFinder::new(production_only.path()).broader_test_command(None),
                None,
                "{source}"
            );
        }

        let sibling_test = project(&[
            ("app.csproj", "<Project />"),
            (
                "tests/tests.csproj",
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
            ),
        ]);
        assert_eq!(
            CommandFinder::new(sibling_test.path()).broader_test_command(Some("src/app.cs")),
            None
        );
    }

    #[test]
    fn a_nested_production_dotnet_project_blocks_an_ancestor_solution() {
        let production_project = project(&[
            ("app.sln", ""),
            ("src/app/Production.csproj", "<Project />"),
        ]);
        let local_test = project(&[
            ("app.sln", ""),
            (
                "tests/Tests.csproj",
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
            ),
        ]);

        assert_eq!(
            CommandFinder::new(production_project.path())
                .broader_test_command(Some("src/app/Service.cs")),
            None
        );
        assert_eq!(
            CommandFinder::new(local_test.path())
                .broader_test_command(Some("tests/Service.cs"))
                .as_deref(),
            Some("cd tests && dotnet test")
        );
    }

    #[test]
    fn a_broader_command_uses_the_target_language_when_runner_manifests_share_a_folder() {
        let project = project(&[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            ("pom.xml", ""),
            ("pyproject.toml", "[tool.pytest.ini_options]\n"),
            (
                "app.csproj",
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
            ),
        ]);

        for (path, expected) in [
            ("src/lib.rs", "cargo test"),
            ("src/app.go", "go test ./..."),
            ("src/app.ts", "npm test"),
            ("src/App.java", "mvn test"),
            ("src/test_app.py", "pytest"),
            ("src/App.cs", "dotnet test"),
        ] {
            assert_eq!(
                CommandFinder::new(project.path())
                    .broader_test_command(Some(path))
                    .as_deref(),
                Some(expected),
                "{path}"
            );
        }
    }

    #[test]
    fn a_broader_command_recognizes_supported_language_extensions() {
        let project = project(&[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            ("pom.xml", ""),
            ("pyproject.toml", "[tool.pytest.ini_options]\n"),
            (
                "app.csproj",
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
            ),
        ]);
        let mut finder = CommandFinder::new(project.path());

        for (extension, expected) in [
            ("rs", Some("cargo test")),
            ("go", Some("go test ./...")),
            ("js", Some("npm test")),
            ("mjs", Some("npm test")),
            ("cjs", Some("npm test")),
            ("jsx", Some("npm test")),
            ("ts", Some("npm test")),
            ("mts", Some("npm test")),
            ("cts", Some("npm test")),
            ("tsx", Some("npm test")),
            ("vue", Some("npm test")),
            ("java", Some("mvn test")),
            ("kt", Some("mvn test")),
            ("kts", Some("mvn test")),
            ("scala", Some("mvn test")),
            ("sc", Some("mvn test")),
            ("py", Some("pytest")),
            ("pyw", Some("pytest")),
            ("cs", Some("dotnet test")),
            ("csx", Some("dotnet test")),
            ("fs", Some("dotnet test")),
            ("fsi", Some("dotnet test")),
            ("fsx", Some("dotnet test")),
            ("vb", Some("dotnet test")),
            ("razor", Some("dotnet test")),
            ("cshtml", Some("dotnet test")),
            ("swift", None),
        ] {
            let path = format!("src/test.{extension}");
            assert_eq!(
                finder.broader_test_command(Some(&path)).as_deref(),
                expected,
                "{extension}"
            );
        }

        assert_eq!(finder.broader_test_command(Some("src/test")), None);
    }

    #[test]
    fn a_nested_package_without_a_test_script_blocks_an_ancestor_suite() {
        let project = project(&[
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            (
                "packages/app/package.json",
                r#"{"scripts":{"build":"node build.js"}}"#,
            ),
        ]);

        assert_eq!(
            CommandFinder::new(project.path())
                .broader_test_command(Some("packages/app/test/app.test.js")),
            None
        );
    }

    #[test]
    fn scoped_cargo_commands_require_nearest_package_evidence() {
        let project = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/member\", \"crates/unresolved\"]\nexclude = [\"vendor/excluded\"]\n",
            ),
            ("crates/member/Cargo.toml", "[package]\nname = \"member\"\n"),
            (
                "vendor/excluded/Cargo.toml",
                "[package]\nname = \"excluded\"\n",
            ),
        ]);
        let mut finder = CommandFinder::new(project.path());

        assert_eq!(
            finder
                .broader_test_command(Some("crates/member/src/lib.rs"))
                .as_deref(),
            Some("cd crates/member && cargo test")
        );
        assert_eq!(
            finder.broader_test_command(Some("crates/unresolved/src/lib.rs")),
            None
        );
        assert_eq!(finder.broader_test_command(Some("other/src/lib.rs")), None);
        assert_eq!(
            finder
                .broader_test_command(Some("vendor/excluded/src/lib.rs"))
                .as_deref(),
            Some("cd vendor/excluded && cargo test")
        );
        assert_eq!(finder.broader_test_command(Some("src/lib.rs")), None);
    }

    #[test]
    fn a_broader_command_without_a_target_requires_one_runner_at_the_root() {
        let configured_root = project(&[
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            ("crates/core/Cargo.toml", "[package]\nname = \"core\"\n"),
        ]);
        let unconfigured_root = project(&[(
            "crates/core/package.json",
            r#"{"scripts":{"test":"node --test"}}"#,
        )]);
        let ambiguous_root = project(&[
            ("Cargo.toml", "[package]\nname = \"app\"\n"),
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
        ]);

        assert_eq!(
            CommandFinder::new(configured_root.path()).broader_test_command(None),
            Some("npm test".into())
        );
        assert_eq!(
            CommandFinder::new(unconfigured_root.path()).broader_test_command(None),
            None
        );
        assert_eq!(
            CommandFinder::new(ambiguous_root.path()).broader_test_command(None),
            None
        );
    }

    #[test]
    fn a_broader_jvm_command_requires_one_build_tool() {
        let project = project(&[
            ("pom.xml", ""),
            ("build.gradle", ""),
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
        ]);
        let mut finder = CommandFinder::new(project.path());

        assert_eq!(finder.broader_test_command(None), None);
        assert_eq!(finder.broader_test_command(Some("src/App.java")), None);
        assert_eq!(
            finder.broader_test_command(Some("src/app.js")).as_deref(),
            Some("npm test")
        );
    }

    #[test]
    fn gradle_broader_command_uses_the_native_windows_wrapper() {
        for (windows, dir, gradlew, gradlew_bat, expected) in [
            (false, "", Some(""), Some(""), "./gradlew test"),
            (true, "", Some(""), Some(""), "gradlew.bat test"),
            (true, "", None, Some(""), "gradlew.bat test"),
            (false, "", None, Some(""), "gradle test"),
            (true, "", None, None, "gradle test"),
            (false, "a/b", Some(""), Some(""), "../../gradlew test"),
            (true, "a/b", Some(""), Some("a"), "..\\gradlew.bat test"),
            (true, "a/b", Some("a/b"), None, "./gradlew test"),
        ] {
            assert_eq!(
                gradle_test_command(windows, dir, gradlew, gradlew_bat),
                expected
            );
        }
    }

    #[test]
    fn a_marker_file_is_read_once_per_folder_for_the_life_of_the_finder() {
        let project = project(&[("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)]);
        let mut finder = CommandFinder::new(project.path());
        let spec = |path| TestSpec {
            language: "typescript",
            kind: "function",
            path,
            line: 1,
            name: "case",
            classes: &[],
            module: None,
            groups: &[],
        };
        let first = finder.command(&spec("a.test.ts"));

        fs::write(
            project.path().join("package.json"),
            r#"{"devDependencies":{"jest":"^29"}}"#,
        )
        .unwrap();
        let second = finder.command(&spec("b.test.ts"));

        assert_eq!(
            first.as_deref(),
            Some(r#"npx vitest run a.test.ts -t "case""#)
        );
        assert_eq!(
            second.as_deref(),
            Some(r#"npx vitest run b.test.ts -t "case""#)
        );
    }

    fn at(
        project: &tempfile::TempDir,
        language: &str,
        kind: &str,
        path: &str,
        name: &str,
    ) -> Option<String> {
        let text = fs::read_to_string(project.path().join(path)).unwrap();
        let line = text
            .lines()
            .position(|line| line.contains(&format!("{name}(")))
            .unwrap()
            + 1;
        CommandFinder::new(project.path()).command(&TestSpec {
            language,
            kind,
            path,
            line,
            name,
            classes: &[],
            module: None,
            groups: &[],
        })
    }

    fn pytest(project: &tempfile::TempDir, path: &str, classes: &[(&str, bool)]) -> Option<String> {
        let classes: Vec<TestClass> = classes
            .iter()
            .map(|(name, test_case)| TestClass {
                name,
                test_case: *test_case,
            })
            .collect();
        CommandFinder::new(project.path()).command(&TestSpec {
            language: "python",
            kind: "method",
            path,
            line: 1,
            name: "test_m",
            classes: &classes,
            module: None,
            groups: &[],
        })
    }

    #[test]
    fn a_go_benchmark_runs_with_the_bench_flag_and_no_test() {
        let module = project(&[("go.mod", "module m\n")]);

        assert_eq!(
            command(&module, "go", "pkg/run_test.go", "BenchmarkRun", None).as_deref(),
            Some("go test ./pkg -bench '^BenchmarkRun$' -run '^$'")
        );
        assert_eq!(
            command(&module, "go", "run_test.go", "Benchmark_snake", None).as_deref(),
            Some("go test ./. -bench '^Benchmark_snake$' -run '^$'")
        );
    }

    #[test]
    fn a_go_fuzz_target_and_a_test_run_by_name_when_the_go_tool_reads_the_name_as_one() {
        let module = project(&[("go.mod", "module m\n")]);

        for name in [
            "TestRun",
            "Test_snake",
            "Test",
            "FuzzRun",
            "Fuzz",
            "TestÜber",
        ] {
            assert_eq!(
                command(&module, "go", "pkg/run_test.go", name, None).as_deref(),
                Some(format!("go test ./pkg -run '^{name}$'").as_str()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_go_function_the_go_tool_does_not_run_by_name_has_no_command() {
        let module = project(&[("go.mod", "module m\n")]);

        for name in [
            "helperNotATest",
            "Testlower",
            "Benchmarkx",
            "TestMain",
            "setupTest",
        ] {
            assert_eq!(
                command(&module, "go", "pkg/run_test.go", name, None),
                None,
                "{name}"
            );
        }
        assert_eq!(command(&module, "go", "pkg/run.go", "TestRun", None), None);
        let method = CommandFinder::new(module.path()).command(&TestSpec {
            language: "go",
            kind: "method",
            path: "pkg/run_test.go",
            line: 1,
            name: "TestSuiteMethod",
            classes: &[],
            module: None,
            groups: &[],
        });
        assert_eq!(method, None);
    }

    #[test]
    fn a_go_example_runs_only_when_its_body_ends_in_an_output_comment() {
        let module = project(&[
            ("go.mod", "module m\n"),
            (
                "pkg/ex_test.go",
                "package pkg\n\nfunc ExampleOne() {\n\tfmt.Println(1)\n\t// Output: 1\n}\n\nfunc ExampleTwo() {\n\tfmt.Println(2)\n\t// Unordered output: 2\n}\n\nfunc ExampleThree() {\n\tfmt.Println(3)\n}\n\nfunc ExampleFour() {\n\tfmt.Println(4)\n\t// output: 4\n}\n\nfunc ExampleFive() { fmt.Println(5) }\n\nfunc ExampleSix() {\n\t// shows 6\n}\n",
            ),
        ]);
        let run = |name: &str| at(&module, "go", "function", "pkg/ex_test.go", name);

        assert_eq!(
            run("ExampleOne").as_deref(),
            Some("go test ./pkg -run '^ExampleOne$'")
        );
        assert_eq!(
            run("ExampleTwo").as_deref(),
            Some("go test ./pkg -run '^ExampleTwo$'")
        );
        assert_eq!(
            run("ExampleFour").as_deref(),
            Some("go test ./pkg -run '^ExampleFour$'")
        );
        for name in ["ExampleThree", "ExampleFive", "ExampleSix"] {
            assert_eq!(run(name), None, "{name}");
        }
    }

    #[test]
    fn a_maven_or_gradle_test_below_the_root_changes_into_the_folder_of_its_build() {
        let maven = project(&[
            ("mvn/pom.xml", "<project/>"),
            ("my app/pom.xml", "<project/>"),
        ]);
        let local_wrapper = project(&[("gr/gradlew", ""), ("gr/build.gradle", "")]);
        let far_wrapper = project(&[("gradlew", ""), ("a/b/build.gradle.kts", "")]);
        let mid_wrapper = project(&[("a/gradlew", ""), ("a/b/build.gradle", "")]);
        let root_build = project(&[("gradlew", ""), ("build.gradle", "")]);

        let run = |project: &tempfile::TempDir, dir: &str| {
            command(
                project,
                "java",
                &format!("{dir}/src/test/java/FooTest.java"),
                "checks",
                Some("FooTest"),
            )
        };

        assert_eq!(
            run(&maven, "mvn").as_deref(),
            Some("cd mvn && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&maven, "my app").as_deref(),
            Some("cd 'my app' && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&local_wrapper, "gr").as_deref(),
            Some("cd gr && ./gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&far_wrapper, "a/b").as_deref(),
            Some("cd a/b && ../../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&mid_wrapper, "a/b").as_deref(),
            Some("cd a/b && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            command(
                &root_build,
                "java",
                "src/test/java/FooTest.java",
                "checks",
                Some("FooTest")
            )
            .as_deref(),
            Some("./gradlew test --tests 'FooTest.checks'")
        );
    }

    #[test]
    fn a_python_test_in_a_folder_with_square_brackets_runs_from_that_folder() {
        let empty = project(&[]);

        assert_eq!(
            command(
                &empty,
                "python",
                "tests/a[b] (c)/test_p.py",
                "test_m",
                Some("TestK")
            )
            .as_deref(),
            Some("cd 'tests/a[b] (c)' && pytest test_p.py::TestK::test_m")
        );
        assert_eq!(
            command(&empty, "python", "a[b]/test_p.py", "test_free", None).as_deref(),
            Some("cd 'a[b]' && pytest test_p.py::test_free")
        );
        assert_eq!(
            command(&empty, "python", "a::b/test_p.py", "test_free", None).as_deref(),
            Some("cd 'a::b' && pytest test_p.py::test_free")
        );
        assert_eq!(
            command(&empty, "python", "tests/a]b/test_p.py", "test_free", None).as_deref(),
            Some("pytest 'tests/a]b/test_p.py'::test_free")
        );
    }

    #[test]
    fn a_python_test_in_a_file_whose_name_holds_square_brackets_has_no_command() {
        let empty = project(&[]);

        assert_eq!(
            command(&empty, "python", "tests/test_[a].py", "test_m", None),
            None
        );
        assert_eq!(
            command(&empty, "python", "tests/test::a.py", "test_m", None),
            None
        );
    }

    #[test]
    fn a_python_test_in_a_class_pytest_does_not_collect_has_no_command() {
        let empty = project(&[]);
        let path = "tests/test_c.py";

        assert_eq!(pytest(&empty, path, &[("Helper", false)]), None);
        assert_eq!(
            pytest(&empty, path, &[("TestOuter", false), ("Helper", false)]),
            None
        );
        assert_eq!(
            pytest(
                &empty,
                path,
                &[
                    ("TestOuter", false),
                    ("Helper", false),
                    ("TestInner", false)
                ]
            ),
            None
        );
        assert_eq!(
            pytest(&empty, path, &[("Testing", false)]).as_deref(),
            Some("pytest tests/test_c.py::Testing::test_m")
        );
    }

    #[test]
    fn a_python_test_case_subclass_is_collected_under_any_name_but_its_nested_classes_are_not() {
        let empty = project(&[]);
        let path = "tests/test_c.py";

        assert_eq!(
            pytest(&empty, path, &[("FooTests", true)]).as_deref(),
            Some("pytest tests/test_c.py::FooTests::test_m")
        );
        assert_eq!(
            pytest(&empty, path, &[("TestOuter", false), ("Inner", true)]).as_deref(),
            Some("pytest tests/test_c.py::TestOuter::Inner::test_m")
        );
        assert_eq!(
            pytest(&empty, path, &[("FooTests", true), ("TestInner", false)]),
            None
        );
        assert_eq!(
            pytest(&empty, path, &[("TestFoo", true), ("TestInner", false)]),
            None
        );
    }

    #[test]
    fn a_rust_test_marked_ignore_runs_with_the_ignored_flag() {
        let cargo = project(&[
            ("Cargo.toml", "[package]\nname = \"p\"\n"),
            (
                "src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    #[test]\n    #[ignore]\n    fn slow() {}\n\n    #[test]\n    #[ignore = \"needs network\"]\n    fn net() {}\n\n    #[ignore] // later\n    #[test]\n    fn flipped() {}\n\n    #[test]\n    #[ignored_by_us]\n    fn custom() {}\n\n    /// #[ignore]\n    #[test]\n    fn documented() {}\n\n    #[test]\n    fn fast() {}\n\n    #[test] #[ignore]\n    fn one_line() {}\n}\n",
            ),
            (
                "tests/it.rs",
                "#[test]\n#[ignore]\nfn slow_it() {}\n\n#[test]\nfn ok_it() {}\n",
            ),
        ]);
        let run = |path: &str, name: &str| at(&cargo, "rust", "function", path, name);

        for name in ["slow", "net", "flipped", "one_line"] {
            assert_eq!(
                run("src/lib.rs", name).as_deref(),
                Some(format!("cargo test -p p {name} -- --ignored").as_str()),
                "{name}"
            );
        }
        for name in ["custom", "documented", "fast"] {
            assert_eq!(
                run("src/lib.rs", name).as_deref(),
                Some(format!("cargo test -p p {name}").as_str()),
                "{name}"
            );
        }
        assert_eq!(
            run("tests/it.rs", "slow_it").as_deref(),
            Some("cargo test -p p --test it slow_it -- --ignored")
        );
        assert_eq!(
            run("tests/it.rs", "ok_it").as_deref(),
            Some("cargo test -p p --test it ok_it")
        );
    }

    #[test]
    fn an_ignored_rust_test_below_the_root_or_without_a_package_keeps_the_ignored_flag() {
        let member = project(&[
            ("crates/n/Cargo.toml", "[package]\nname = \"inner\"\n"),
            ("crates/n/src/lib.rs", "#[test]\n#[ignore]\nfn slow() {}\n"),
        ]);
        let bare = project(&[("src/lib.rs", "#[test]\n#[ignore]\nfn slow() {}\n")]);

        assert_eq!(
            at(&member, "rust", "function", "crates/n/src/lib.rs", "slow").as_deref(),
            Some("cd crates/n && cargo test -p inner slow -- --ignored")
        );
        assert_eq!(
            at(&bare, "rust", "function", "src/lib.rs", "slow").as_deref(),
            Some("cargo test slow -- --ignored")
        );
    }

    #[test]
    fn a_node_test_runs_only_the_named_test_even_when_the_name_holds_regular_expression_characters()
    {
        let files = [("package.json", r#"{"scripts":{"test":"node --test"}}"#)];

        assert_eq!(
            script(&files, "a.test.js", "sums a.b (1+2) [3] $4").as_deref(),
            Some(r#"node --test-name-pattern "^sums a\\.b \\(1\\+2\\) \\[3\\] \\\$4\$" a.test.js"#)
        );
        assert_eq!(
            script(&files, "a.test.js", "/api/users").as_deref(),
            Some(r#"node --test-name-pattern "^/api/users\$" a.test.js"#)
        );
        assert_eq!(
            script(&files, "a.test.js", "handles ${kind} kind").as_deref(),
            Some(r#"node --test-name-pattern "^handles .* kind\$" a.test.js"#)
        );
    }

    struct Spec<'a> {
        language: &'a str,
        kind: &'a str,
        path: &'a str,
        line: usize,
        name: &'a str,
        classes: &'a [&'a str],
        module: Option<&'a str>,
        groups: &'a [&'a str],
    }

    impl Spec<'_> {
        fn run(&self, project: &tempfile::TempDir) -> Option<String> {
            let classes: Vec<TestClass> = self
                .classes
                .iter()
                .map(|name| TestClass {
                    name,
                    test_case: false,
                })
                .collect();
            CommandFinder::new(project.path()).command(&TestSpec {
                language: self.language,
                kind: self.kind,
                path: self.path,
                line: self.line,
                name: self.name,
                classes: &classes,
                module: self.module,
                groups: self.groups,
            })
        }
    }

    fn spec<'a>(language: &'a str, kind: &'a str, path: &'a str, name: &'a str) -> Spec<'a> {
        Spec {
            language,
            kind,
            path,
            line: 3,
            name,
            classes: &[],
            module: None,
            groups: &[],
        }
    }

    #[test]
    fn a_targeted_gradle_command_uses_the_platform_wrapper_of_the_broader_command() {
        let both = project(&[
            ("gradlew", ""),
            ("gradlew.bat", ""),
            ("app/build.gradle", ""),
        ]);
        let windows_only = project(&[("gradlew.bat", ""), ("app/build.gradle", "")]);
        let test = Spec {
            classes: &["FooTest"],
            ..spec("java", "method", "app/src/test/java/FooTest.java", "checks")
        };
        let (both_tool, windows_only_tool) = if cfg!(windows) {
            ("..\\gradlew.bat", "..\\gradlew.bat")
        } else {
            ("../gradlew", "gradle")
        };

        assert_eq!(
            test.run(&both),
            Some(format!(
                "cd app && {both_tool} test --tests 'FooTest.checks'"
            ))
        );
        assert_eq!(
            test.run(&windows_only),
            Some(format!(
                "cd app && {windows_only_tool} test --tests 'FooTest.checks'"
            ))
        );
        for (windows, dir, gradlew, gradlew_bat, expected) in [
            (true, "app", Some(""), Some(""), "..\\gradlew.bat"),
            (false, "app", Some(""), Some(""), "../gradlew"),
            (true, "app", None, Some("app"), "gradlew.bat"),
            (false, "app", None, Some(""), "gradle"),
        ] {
            assert_eq!(gradle_tool(windows, dir, gradlew, gradlew_bat), expected);
        }
    }

    #[test]
    fn a_gradle_test_in_a_multi_project_build_runs_only_its_project_task_from_the_settings_folder()
    {
        let build = project(&[
            ("gradlew", ""),
            (
                "settings.gradle.kts",
                "include(\":lib\")\ninclude(\":lib:extra\")\n",
            ),
            ("build.gradle.kts", ""),
            ("lib/build.gradle.kts", ""),
            ("lib/extra/build.gradle.kts", ""),
        ]);
        let in_lib = Spec {
            classes: &["LibTest"],
            ..spec("kotlin", "method", "lib/src/test/kotlin/LibTest.kt", "adds")
        };
        let in_extra = Spec {
            path: "lib/extra/src/test/kotlin/ExtraTest.kt",
            classes: &["ExtraTest"],
            ..in_lib
        };
        let in_root = Spec {
            path: "src/test/kotlin/RootTest.kt",
            classes: &["RootTest"],
            ..in_lib
        };
        let tool = "./gradlew";
        let mut finder = CommandFinder::new(build.path());

        assert_eq!(
            in_lib.run(&build),
            Some(format!("{tool} :lib:test --tests 'LibTest.adds'"))
        );
        assert_eq!(
            in_extra.run(&build),
            Some(format!("{tool} :lib:extra:test --tests 'ExtraTest.adds'"))
        );
        assert_eq!(
            in_root.run(&build),
            Some(format!("{tool} :test --tests 'RootTest.adds'"))
        );
        assert_eq!(
            finder.broader_test_command(Some("lib/src/test/kotlin/LibTest.kt")),
            Some(format!("{tool} :lib:test"))
        );
        assert_eq!(
            finder.broader_test_command(None),
            Some(format!("{tool} test"))
        );
    }

    #[test]
    fn a_gradle_project_the_settings_cannot_name_has_no_command() {
        let dynamic = project(&[
            (
                "settings.gradle",
                "file('libs').eachDir { include \":libs:${it.name}\" }\n",
            ),
            ("libs/a/build.gradle", ""),
        ]);
        let unlisted = project(&[
            ("settings.gradle", "include 'app'\n"),
            ("tools/build.gradle", ""),
        ]);
        let both_settings = project(&[
            ("settings.gradle", "include 'app'\n"),
            ("settings.gradle.kts", "include(\"app\")\n"),
            ("app/build.gradle", ""),
        ]);
        let test = |path| Spec {
            classes: &["FooTest"],
            ..spec("java", "method", path, "checks")
        };

        assert_eq!(
            test("libs/a/src/test/java/FooTest.java").run(&dynamic),
            None
        );
        assert_eq!(
            test("tools/src/test/java/FooTest.java").run(&unlisted),
            None
        );
        assert_eq!(
            test("app/src/test/java/FooTest.java").run(&both_settings),
            None
        );
        assert_eq!(
            CommandFinder::new(unlisted.path())
                .broader_test_command(Some("tools/src/test/java/FooTest.java")),
            None
        );
    }

    #[test]
    fn a_gradle_buildsrc_folder_is_its_own_build_beside_a_multi_project_settings_file() {
        let build = project(&[
            ("gradlew", ""),
            ("settings.gradle.kts", "include(\":a\")\n"),
            ("a/build.gradle.kts", ""),
            ("buildSrc/build.gradle.kts", ""),
            ("buildSrc/sub/build.gradle.kts", ""),
        ]);
        let with_settings = project(&[
            ("gradlew", ""),
            ("settings.gradle.kts", "include(\":a\")\n"),
            ("buildSrc/settings.gradle.kts", "include(\":plugins\")\n"),
            ("buildSrc/build.gradle.kts", ""),
            ("buildSrc/plugins/build.gradle.kts", ""),
        ]);
        let test = |path| Spec {
            classes: &["LibTest"],
            ..spec("java", "method", path, "computes")
        };
        let (tool, up) = ("./gradlew", "../gradlew");

        assert_eq!(
            test("buildSrc/src/test/java/p/LibTest.java").run(&build),
            Some(format!(
                "cd buildSrc && {up} test --tests 'LibTest.computes'"
            ))
        );
        assert_eq!(
            CommandFinder::new(build.path())
                .broader_test_command(Some("buildSrc/src/test/java/p/LibTest.java")),
            Some(format!("cd buildSrc && {up} test"))
        );
        assert_eq!(
            test("buildSrc/sub/src/test/java/p/LibTest.java").run(&build),
            None
        );
        assert_eq!(
            test("a/src/test/java/p/LibTest.java").run(&build),
            Some(format!("{tool} :a:test --tests 'LibTest.computes'"))
        );
        assert_eq!(
            test("buildSrc/plugins/src/test/java/p/LibTest.java").run(&with_settings),
            Some(format!(
                "cd buildSrc && {up} :plugins:test --tests 'LibTest.computes'"
            ))
        );
    }

    #[test]
    fn a_gradle_project_that_a_sibling_settings_file_includes_flat_runs_from_that_folder() {
        let flat = project(&[
            ("gradlew", ""),
            ("build/settings.gradle", "includeFlat 'x', 'y'\n"),
            ("x/build.gradle.kts", ""),
            ("y/build.gradle.kts", ""),
            ("solo/build.gradle.kts", ""),
        ]);
        let two_masters = project(&[
            ("one/settings.gradle", "includeFlat 'x'\n"),
            ("two/settings.gradle", "includeFlat 'x'\n"),
            ("x/build.gradle", ""),
        ]);
        let conditional = project(&[
            (
                "build/settings.gradle",
                "if (System.getenv('CI'))\n    includeFlat 'x'\n",
            ),
            ("x/build.gradle", ""),
        ]);
        let test = |path| Spec {
            classes: &["LibTest"],
            ..spec("java", "method", path, "computes")
        };
        let up = "../gradlew";

        assert_eq!(
            test("x/src/test/java/p/LibTest.java").run(&flat),
            Some(format!(
                "cd build && {up} :x:test --tests 'LibTest.computes'"
            ))
        );
        assert_eq!(
            CommandFinder::new(flat.path())
                .broader_test_command(Some("y/src/test/java/p/LibTest.java")),
            Some(format!("cd build && {up} :y:test"))
        );
        assert_eq!(
            test("solo/src/test/java/p/LibTest.java").run(&flat),
            Some(format!("cd solo && {up} test --tests 'LibTest.computes'"))
        );
        assert_eq!(
            test("x/src/test/java/p/LibTest.java").run(&two_masters),
            None
        );
        assert_eq!(
            test("x/src/test/java/p/LibTest.java").run(&conditional),
            None
        );
    }

    #[test]
    fn a_kotest_label_in_a_gradle_subproject_runs_its_project_task() {
        let build = project(&[
            ("gradlew", ""),
            ("settings.gradle", "include ':spec'\n"),
            ("spec/build.gradle.kts", ""),
            (
                "spec/src/test/kotlin/CalcSpec.kt",
                "package calc\n\nimport io.kotest.core.spec.style.StringSpec\n",
            ),
        ]);
        let label = Spec {
            classes: &["CalcSpec"],
            ..spec(
                "kotlin",
                "function",
                "spec/src/test/kotlin/CalcSpec.kt",
                "adds two",
            )
        };
        let tool = "./gradlew";

        assert_eq!(
            label.run(&build),
            Some(format!(
                "{tool} :spec:test --tests 'calc.CalcSpec.adds two'"
            ))
        );
    }

    #[test]
    fn a_kotlin_or_scala_test_label_has_no_maven_or_gradle_command() {
        let maven = project(&[("pom.xml", "<project/>")]);
        let gradle = project(&[("build.gradle.kts", "")]);
        let label = Spec {
            classes: &["CalcSpec"],
            ..spec(
                "kotlin",
                "function",
                "src/test/kotlin/CalcSpec.kt",
                "adds two",
            )
        };
        let method = Spec {
            kind: "method",
            ..label
        };

        assert_eq!(label.run(&maven), None);
        assert_eq!(label.run(&gradle), None);
        assert_eq!(
            method.run(&gradle).as_deref(),
            Some("gradle test --tests 'CalcSpec.adds two'")
        );
    }

    #[test]
    fn an_sbt_build_without_scalatest_runs_the_whole_class() {
        let sbt = project(&[("build.sbt", "libraryDependencies += munit\n")]);
        let ambiguous = project(&[("build.sbt", ""), ("pom.xml", "<project/>")]);
        let label = Spec {
            classes: &["Outer", "CalcSuite"],
            ..spec(
                "scala",
                "function",
                "src/test/scala/CalcSuite.scala",
                "adds",
            )
        };

        assert_eq!(
            label.run(&sbt).as_deref(),
            Some("sbt 'testOnly *Outer$CalcSuite'")
        );
        assert_eq!(
            CommandFinder::new(sbt.path())
                .broader_test_command(Some("src/test/scala/CalcSuite.scala"))
                .as_deref(),
            Some("sbt test")
        );
        assert_eq!(
            CommandFinder::new(ambiguous.path())
                .broader_test_command(Some("src/test/scala/CalcSuite.scala")),
            None
        );
    }

    #[test]
    fn an_fsharp_or_vbnet_module_test_filters_by_its_module_and_csharp_needs_a_class() {
        let empty = project(&[]);

        assert_eq!(
            Spec {
                module: Some("Demo.Tests"),
                ..spec("fsharp", "function", "Tests.fs", "``adds \"two\"``")
            }
            .run(&empty)
            .as_deref(),
            Some(r#"dotnet test --filter "FullyQualifiedName~Demo.Tests.adds \"two\"""#)
        );
        assert_eq!(
            Spec {
                module: Some("CalcModule"),
                ..spec("vbnet", "method", "Calc.vb", "Adds")
            }
            .run(&empty)
            .as_deref(),
            Some(r#"dotnet test --filter "FullyQualifiedName~CalcModule.Adds""#)
        );
        assert_eq!(
            Spec {
                module: Some("Demo"),
                ..spec("csharp", "method", "Calc.cs", "Adds")
            }
            .run(&empty),
            None
        );
        assert_eq!(
            spec("fsharp", "function", "Tests.fs", "adds").run(&empty),
            None
        );
    }

    #[test]
    fn ruby_tests_follow_the_nearest_ruby_project_only() {
        let nested = project(&[
            (".rspec", "--require spec_helper\n"),
            ("engine/Gemfile", "gem \"rake\"\n"),
        ]);
        let plain_rspec = project(&[(".rspec", "")]);
        let rake_function = project(&[("Rakefile", "Rake::TestTask.new\n")]);
        let example = spec("ruby", "function", "engine/spec/calc_spec.rb", "adds two");

        assert_eq!(example.run(&nested), None);
        assert_eq!(
            Spec {
                path: "spec/calc spec_spec.rb",
                ..example
            }
            .run(&plain_rspec)
            .as_deref(),
            Some("rspec 'spec/calc spec_spec.rb':3")
        );
        assert_eq!(
            spec("ruby", "function", "test/calc_test.rb", "adds two").run(&rake_function),
            None
        );
    }

    #[test]
    fn a_swift_test_in_nested_types_names_each_type_in_its_filter() {
        let package = project(&[("Package.swift", "")]);

        assert_eq!(
            Spec {
                classes: &["Outer", "Inner"],
                ..spec("swift", "method", "Tests/A/CalcTests.swift", "adds")
            }
            .run(&package)
            .as_deref(),
            Some(r"swift test --filter 'Outer/Inner/adds\b'")
        );
    }

    #[test]
    fn a_dart_name_with_an_escape_or_interpolation_runs_the_whole_file() {
        let dart = project(&[("pkg/pubspec.yaml", "dev_dependencies:\n  test: any\n")]);
        let nested = project(&[
            ("pubspec.yaml", "dev_dependencies:\n  test: any\n"),
            ("tool/pubspec.yaml", "name: tool\n"),
        ]);

        assert_eq!(
            spec("dart", "function", "pkg/test/a_test.dart", "adds $n").run(&dart),
            Some("cd pkg && dart test test/a_test.dart".to_string())
        );
        assert_eq!(
            spec("dart", "function", "tool/test/a_test.dart", "adds").run(&nested),
            None
        );
    }

    #[test]
    fn a_ctest_command_needs_a_googletest_macro_and_exactly_one_build_folder() {
        let cmake = "gtest_discover_tests(calc_test)\n";
        let source = "#include <gtest/gtest.h>\n\nTEST_P(CalcTest, Adds) {}\nTYPED_TEST(CalcTest, Typed) {}\n";
        let one = project(&[
            ("CMakeLists.txt", cmake),
            ("build/CTestTestfile.cmake", ""),
            ("tests/calc_test.cpp", source),
        ]);
        let two = project(&[
            ("CMakeLists.txt", cmake),
            ("build/CTestTestfile.cmake", ""),
            ("out/CTestTestfile.cmake", ""),
            ("tests/calc_test.cpp", source),
        ]);
        let undiscovered = project(&[
            ("CMakeLists.txt", "add_test(NAME calc COMMAND calc_test)\n"),
            ("build/CTestTestfile.cmake", ""),
            ("tests/calc_test.cpp", source),
        ]);
        let param = spec("cpp", "function", "tests/calc_test.cpp", "CalcTest.Adds");
        let typed = Spec {
            line: 4,
            ..spec("cpp", "function", "tests/calc_test.cpp", "CalcTest.Typed")
        };

        assert_eq!(
            param.run(&one).as_deref(),
            Some(r"ctest --test-dir build -R '/CalcTest\.Adds/'")
        );
        assert_eq!(typed.run(&one), None);
        assert_eq!(param.run(&two), None);
        assert_eq!(param.run(&undiscovered), None);
    }

    #[test]
    fn phpunit_runs_globally_without_composer_and_not_at_all_without_configuration() {
        let config = project(&[("phpunit.xml.dist", "<phpunit/>")]);
        let composer = project(&[("composer.json", "{\"require\": {}}")]);
        let test = Spec {
            classes: &["CalcTest"],
            ..spec("php", "method", "tests/CalcTest.php", "testAdds")
        };

        assert_eq!(
            test.run(&config).as_deref(),
            Some(r"phpunit --filter 'CalcTest::testAdds\b' tests/CalcTest.php")
        );
        assert_eq!(test.run(&composer), None);
    }

    #[test]
    fn rebar3_runs_an_eunit_generator_and_nothing_for_a_helper() {
        let rebar = project(&[("apps/calc/rebar.config", "")]);
        let path = "apps/calc/test/calc_tests.erl";

        assert_eq!(
            spec("erlang", "function", path, "adds_test_")
                .run(&rebar)
                .as_deref(),
            Some("cd apps/calc && rebar3 eunit --generator='calc_tests:adds_test_'")
        );
        assert_eq!(spec("erlang", "function", path, "setup").run(&rebar), None);
    }

    #[test]
    fn busted_needs_a_busted_file_or_a_rockspec_that_names_busted() {
        let rockspec = project(&[("calc-1.0-1.rockspec", "test = { type = \"busted\" }\n")]);
        let other = project(&[("calc-1.0-1.rockspec", "build = {}\n")]);
        let test = spec("lua", "function", "spec/calc_spec.lua", "adds 100%");

        assert_eq!(
            test.run(&rockspec).as_deref(),
            Some("busted spec/calc_spec.lua --filter='adds 100%%$'")
        );
        assert_eq!(test.run(&other), None);
    }

    #[test]
    fn testthat_needs_a_description_that_names_testthat() {
        let other = project(&[("DESCRIPTION", "Package: calc\nImports: rlang\n")]);
        let nested = project(&[
            ("DESCRIPTION", "Package: calc\nSuggests: testthat\n"),
            ("sub/DESCRIPTION", "Package: sub\n"),
        ]);

        assert_eq!(
            spec("r", "function", "tests/testthat/test-a.R", "adds").run(&other),
            None
        );
        assert_eq!(
            spec("r", "function", "sub/tests/testthat/test-a.R", "adds").run(&nested),
            None
        );
    }

    #[test]
    fn pester_and_bats_read_the_runner_from_the_file_name() {
        let empty = project(&[]);

        assert_eq!(
            spec("powershell", "function", "tests/Calc.ps1", "adds").run(&empty),
            None
        );
        assert_eq!(
            spec("powershell", "function", "Calc.Tests.ps1", "it's $x").run(&empty),
            Some(r#"pwsh -NoProfile -Command "Invoke-Pester -Path 'Calc.Tests.ps1' -FullNameFilter '*.it''s \$x'""#.to_string())
        );
        assert_eq!(
            spec("bash", "function", "test/calc.sh", "adds").run(&empty),
            None
        );
        assert_eq!(
            spec("bash", "function", "test/calc.bats", "a.b|c's").run(&empty),
            Some(r"bats test/calc.bats --filter '^a\.b\|c'\''s$'".to_string())
        );
    }

    #[test]
    fn gut_needs_the_addon_in_the_godot_project() {
        let bare = project(&[("project.godot", "")]);

        assert_eq!(
            spec("gdscript", "method", "test/test_calc.gd", "test_adds").run(&bare),
            None
        );
    }

    #[test]
    fn a_zig_test_needs_a_name_without_escapes_and_a_build_zig() {
        let source = "test {\n}\ntest \"a\\tb\" {\n}\ntest @\"odd\" {\n}\ntest \"plain\" {\n}\n";
        let built = project(&[("build.zig", ""), ("src/a.zig", source)]);
        let bare = project(&[("src/a.zig", source)]);
        let at = |line, name| Spec {
            line,
            ..spec("zig", "function", "src/a.zig", name)
        };

        assert_eq!(at(1, "test").run(&built), None);
        assert_eq!(at(3, "a\\tb").run(&built), None);
        assert_eq!(at(5, "odd").run(&built), None);
        assert_eq!(
            at(7, "plain").run(&built).as_deref(),
            Some("zig test src/a.zig --test-filter 'test.plain'")
        );
        assert_eq!(at(7, "plain").run(&bare), None);
    }

    #[test]
    fn a_qml_test_takes_the_literal_name_of_its_innermost_test_case() {
        let source = "TestCase {\n    name: \"Outer\"\n    TestCase {\n        name: \"Inner\" // nested\n        function test_inner() {}\n    }\n    function test_outer() {}\n}\nTestCase {\n    name: caseName\n    function test_computed() {}\n}\nTestCase {\n    name: \"A::B\"\n    function test_colons() {}\n}\n";
        let files = project(&[("tst_cases.qml", source), ("cases.qml", source)]);
        let at = |path, line, name| Spec {
            line,
            ..spec("qml", "function", path, name)
        };

        assert_eq!(
            at("tst_cases.qml", 5, "test_inner").run(&files).as_deref(),
            Some("qmltestrunner -platform offscreen -input tst_cases.qml 'Inner::test_inner'")
        );
        assert_eq!(
            at("tst_cases.qml", 7, "test_outer").run(&files).as_deref(),
            Some("qmltestrunner -platform offscreen -input tst_cases.qml 'Outer::test_outer'")
        );
        assert_eq!(at("tst_cases.qml", 11, "test_computed").run(&files), None);
        assert_eq!(at("tst_cases.qml", 15, "test_colons").run(&files), None);
        assert_eq!(at("cases.qml", 5, "test_inner").run(&files), None);
    }

    #[test]
    fn a_ceedling_test_needs_the_test_file_prefix_of_its_project() {
        let unity = "#include \"unity.h\"\nvoid test_adds(void) {}\n";
        let custom = project(&[
            ("project.yml", ":project:\n  :test_file_prefix: check_\n"),
            ("test/check_calc.c", unity),
            ("test/test_calc.c", unity),
        ]);
        let other_yaml = project(&[("project.yml", "name: calc\n"), ("test/test_calc.c", unity)]);

        assert_eq!(
            spec("c", "function", "test/check_calc.c", "test_adds")
                .run(&custom)
                .as_deref(),
            Some("ceedling test:check_calc.c --test-case=test_adds")
        );
        assert_eq!(
            spec("c", "function", "test/test_calc.c", "test_adds").run(&custom),
            None
        );
        assert_eq!(
            spec("c", "function", "test/test_calc.c", "test_adds").run(&other_yaml),
            None
        );
    }

    #[test]
    fn a_ctest_command_for_c_needs_one_executable_one_test_and_enabled_testing() {
        let source = "#include \"unity.h\"\nvoid test_adds(void) {}\n";
        let with = |cmake: &str| {
            project(&[
                ("CMakeLists.txt", cmake),
                ("build/CTestTestfile.cmake", ""),
                ("tests/calc.c", source),
            ])
        };
        let test = spec("c", "function", "tests/calc.c", "test_adds");

        assert_eq!(
            test.run(&with(
                "ENABLE_TESTING()\nADD_EXECUTABLE(calc \"tests/calc.c\")\nadd_test(NAME calc-unit COMMAND calc --verbose)\n"
            ))
            .as_deref(),
            Some("ctest --test-dir build -R '^calc-unit$'")
        );
        assert_eq!(
            test.run(&with(
                "add_executable(calc tests/calc.c)\nadd_test(NAME calc COMMAND calc)\n"
            )),
            None
        );
        assert_eq!(
            test.run(&with(
                "include(CTest)\nadd_executable(a tests/calc.c)\nadd_executable(b tests/calc.c)\nadd_test(NAME a COMMAND a)\n"
            )),
            None
        );
        assert_eq!(
            test.run(&with(
                "enable_testing()\nadd_executable(calc ${SOURCES})\nadd_test(NAME calc COMMAND calc)\n"
            )),
            None
        );
        assert_eq!(
            test.run(&with(
                "enable_testing() # add_executable(calc tests/calc.c)\nadd_test(NAME calc COMMAND calc)\n"
            )),
            None
        );
    }

    #[test]
    fn a_check_test_case_comes_from_the_one_tcase_that_registers_the_test() {
        let lines = |text: &str| text.lines().map(str::to_string).collect::<Vec<_>>();
        let one = lines(
            "TCase *core = tcase_create(\"Core\");\ntcase_add_test(core, adds);\ntcase_add_loop_test(core, adds, 0, 3);\n",
        );
        let two = lines(
            "TCase *a = tcase_create(\"A\");\nTCase *b = tcase_create(\"B\");\ntcase_add_test(a, adds);\ntcase_add_test(b, adds);\n",
        );

        assert_eq!(check_test_case(&one, "adds").as_deref(), Some("Core"));
        assert_eq!(check_test_case(&one, "subtracts"), None);
        assert_eq!(check_test_case(&two, "adds"), None);
    }

    #[test]
    fn a_kotest_label_needs_a_kotest_import_and_names_its_groups() {
        let gradle = project(&[
            ("build.gradle.kts", ""),
            (
                "src/test/kotlin/CalcSpec.kt",
                "import io.kotest.core.spec.style.FunSpec\n",
            ),
            ("src/test/kotlin/PlainSpec.kt", "class PlainSpec\n"),
        ]);
        let label = Spec {
            classes: &["CalcSpec"],
            groups: &["math", "sums"],
            ..spec(
                "kotlin",
                "function",
                "src/test/kotlin/CalcSpec.kt",
                "adds two",
            )
        };
        let plain = Spec {
            path: "src/test/kotlin/PlainSpec.kt",
            ..label
        };

        assert_eq!(
            label.run(&gradle).as_deref(),
            Some("gradle test --tests 'CalcSpec.math -- sums -- adds two'")
        );
        assert_eq!(plain.run(&gradle), None);
    }

    #[test]
    fn a_test_unit_file_needs_the_gem_when_a_gemfile_exists() {
        let source = "require 'test/unit'\nclass TestCalc < Test::Unit::TestCase\nend\n";
        let gemspec = project(&[
            ("Gemfile", "gemspec\n"),
            (
                "calc.gemspec",
                "spec.add_development_dependency \"test-unit\"\n",
            ),
            ("test/test_calc.rb", source),
        ]);
        let minitest = project(&[
            ("Rakefile", ""),
            (
                "test/test_calc.rb",
                "require \"minitest/autorun\"\nclass TestCalc < Minitest::Test\nend\n",
            ),
        ]);
        let method = Spec {
            classes: &["TestCalc"],
            ..spec("ruby", "method", "test/test_calc.rb", "test_adds")
        };

        assert_eq!(
            method.run(&gemspec).as_deref(),
            Some("bundle exec ruby -Ilib -Itest test/test_calc.rb '--name=test_adds'")
        );
        assert_eq!(method.run(&minitest), None);
    }

    #[test]
    fn cmake_commands_skip_comments_and_unquote_arguments() {
        assert_eq!(
            cmake_commands(
                "# add_test(x y)\nAdd_Test (NAME \"a b\" COMMAND $<TARGET_FILE:t>) # c\nset(V (x))\n"
            ),
            vec![
                (
                    "add_test".to_string(),
                    vec![
                        "NAME".to_string(),
                        "a b".to_string(),
                        "COMMAND".to_string(),
                        "$<TARGET_FILE:t>".to_string()
                    ]
                ),
                ("set".to_string(), vec!["V".to_string(), "(x)".to_string()]),
            ]
        );
    }
}
