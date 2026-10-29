use code_kb_core::{
    LabeledSite, ReferenceReport, SiteLabel, Workspace, find_julie_extract_binary, open_read_only,
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

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path)).unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn dump(report: &ReferenceReport) -> String {
    report
        .sites
        .iter()
        .map(|site| {
            format!(
                "{} {}:{} {}\n",
                site.label.as_str(),
                site.site.path,
                site.site.start_line.unwrap_or(0),
                site.basis
            )
        })
        .collect()
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

struct LoadCase {
    files: &'static [(&'static str, &'static str)],
    name: &'static str,
    loaded: &'static str,
    other: &'static str,
    caller: &'static str,
    call: &'static str,
}

const LOAD_CASES: &[LoadCase] = &[
    LoadCase {
        files: &[
            ("sh/lib/common.sh", "greet() {\n  echo lib\n}\n"),
            ("sh/other/alt.sh", "greet() {\n  echo other\n}\n"),
            (
                "sh/run.sh",
                "source ./lib/common.sh\n\nrun() {\n  greet\n}\n",
            ),
        ],
        name: "greet",
        loaded: "sh/lib/common.sh",
        other: "sh/other/alt.sh",
        caller: "sh/run.sh",
        call: "  greet",
    },
    LoadCase {
        files: &[
            ("lib/common.sh", "greet() {\n  echo lib\n}\n"),
            ("other/alt.sh", "greet() {\n  echo other\n}\n"),
            ("run.sh", ". lib/common.sh\n\nrun() {\n  greet\n}\n"),
        ],
        name: "greet",
        loaded: "lib/common.sh",
        other: "other/alt.sh",
        caller: "run.sh",
        call: "  greet",
    },
    LoadCase {
        files: &[
            ("test/helpers.bash", "make_fixture() {\n  echo lib\n}\n"),
            ("other/alt.sh", "make_fixture() {\n  echo other\n}\n"),
            (
                "test/run.bats",
                "load helpers\n\n@test \"runs\" {\n  make_fixture\n}\n",
            ),
        ],
        name: "make_fixture",
        loaded: "test/helpers.bash",
        other: "other/alt.sh",
        caller: "test/run.bats",
        call: "  make_fixture",
    },
    LoadCase {
        files: &[
            ("ps/lib/Common.ps1", "function Get-Thing {\n  1\n}\n"),
            ("ps/other/Alt.ps1", "function Get-Thing {\n  2\n}\n"),
            (
                "ps/run.ps1",
                ". \"$PSScriptRoot/lib/Common.ps1\"\n\nfunction Invoke-Run {\n  Get-Thing\n}\n",
            ),
        ],
        name: "Get-Thing",
        loaded: "ps/lib/Common.ps1",
        other: "ps/other/Alt.ps1",
        caller: "ps/run.ps1",
        call: "  Get-Thing",
    },
    LoadCase {
        files: &[
            ("ps/Modules/Tools.psm1", "function Get-Tool {\n  1\n}\n"),
            ("ps/other/Alt.ps1", "function Get-Tool {\n  2\n}\n"),
            (
                "ps/run.ps1",
                "Import-Module ./Modules/Tools.psm1\n\nfunction Invoke-Run {\n  Get-Tool\n}\n",
            ),
        ],
        name: "Get-Tool",
        loaded: "ps/Modules/Tools.psm1",
        other: "ps/other/Alt.ps1",
        caller: "ps/run.ps1",
        call: "  Get-Tool",
    },
    LoadCase {
        files: &[
            ("R/helpers.R", "helper <- function() 1\n"),
            ("R/other.R", "helper <- function() 2\n"),
            (
                "R/main.R",
                "source(\"R/helpers.R\")\n\nrun <- function() {\n  helper()\n}\n",
            ),
        ],
        name: "helper",
        loaded: "R/helpers.R",
        other: "R/other.R",
        caller: "R/main.R",
        call: "  helper()",
    },
    LoadCase {
        files: &[
            ("php/lib/x.php", "<?php\nfunction xhelper() { return 1; }\n"),
            (
                "php/other/y.php",
                "<?php\nfunction xhelper() { return 2; }\n",
            ),
            (
                "php/main.php",
                "<?php\nrequire_once __DIR__ . '/lib/x.php';\n\nfunction run() {\n  return xhelper();\n}\n",
            ),
        ],
        name: "xhelper",
        loaded: "php/lib/x.php",
        other: "php/other/y.php",
        caller: "php/main.php",
        call: "xhelper();",
    },
    LoadCase {
        files: &[
            ("php/lib/x.php", "<?php\nfunction xhelper() { return 1; }\n"),
            (
                "php/other/y.php",
                "<?php\nfunction xhelper() { return 2; }\n",
            ),
            (
                "php/main.php",
                "<?php\ninclude 'lib/x.php';\n\nfunction run() {\n  return xhelper();\n}\n",
            ),
        ],
        name: "xhelper",
        loaded: "php/lib/x.php",
        other: "php/other/y.php",
        caller: "php/main.php",
        call: "xhelper();",
    },
    LoadCase {
        files: &[
            ("rb/lib/helper.rb", "def helper_fn\n  1\nend\n"),
            ("rb/other/alt.rb", "def helper_fn\n  2\nend\n"),
            (
                "rb/main.rb",
                "require_relative \"lib/helper\"\n\ndef run\n  helper_fn\nend\n",
            ),
        ],
        name: "helper_fn",
        loaded: "rb/lib/helper.rb",
        other: "rb/other/alt.rb",
        caller: "rb/main.rb",
        call: "  helper_fn",
    },
    LoadCase {
        files: &[
            ("lib/helper.rb", "def helper_fn\n  1\nend\n"),
            ("other/alt.rb", "def helper_fn\n  2\nend\n"),
            (
                "bin/main.rb",
                "require \"helper\"\n\ndef run\n  helper_fn\nend\n",
            ),
        ],
        name: "helper_fn",
        loaded: "lib/helper.rb",
        other: "other/alt.rb",
        caller: "bin/main.rb",
        call: "  helper_fn",
    },
    LoadCase {
        files: &[
            ("tasks/setup.rb", "def helper_fn\n  1\nend\n"),
            ("other/alt.rb", "def helper_fn\n  2\nend\n"),
            (
                "main.rb",
                "load \"tasks/setup.rb\"\n\ndef run\n  helper_fn\nend\n",
            ),
        ],
        name: "helper_fn",
        loaded: "tasks/setup.rb",
        other: "other/alt.rb",
        caller: "main.rb",
        call: "  helper_fn",
    },
    LoadCase {
        files: &[
            ("sh/lib/base.sh", "greet() {\n  echo base\n}\n"),
            ("sh/lib/common.sh", "source \"${0%/*}/x.sh\" 2>/dev/null\n"),
            ("sh/lib/all.sh", "source ./base.sh\n"),
            ("sh/other/alt.sh", "greet() {\n  echo other\n}\n"),
            ("sh/run.sh", "source ./lib/all.sh\n\nrun() {\n  greet\n}\n"),
        ],
        name: "greet",
        loaded: "sh/lib/base.sh",
        other: "sh/other/alt.sh",
        caller: "sh/run.sh",
        call: "  greet",
    },
];

#[test]
fn a_bare_call_reaches_the_top_level_function_of_a_file_its_file_loads() {
    for case in LOAD_CASES {
        let repo = scanned_repo(case.files);
        let line = line_of(&repo, case.caller, case.call);

        assert_site(
            &report(&repo, case.name, case.loaded),
            case.caller,
            line,
            SiteLabel::Proven,
            "import",
        );
        assert_site(
            &report(&repo, case.name, case.other),
            case.caller,
            line,
            SiteLabel::Excluded,
            &format!("the file loads `{}` from `{}`", case.name, case.loaded),
        );
    }
}

const UNDECIDED_LOADS: &[LoadCase] = &[
    LoadCase {
        files: &[
            ("sh/lib/common.sh", "greet() {\n  echo lib\n}\n"),
            ("sh/other/alt.sh", "greet() {\n  echo other\n}\n"),
            (
                "sh/run.sh",
                "DIR=\"$(dirname \"$0\")\"\nsource \"$DIR/lib/common.sh\"\n\nrun() {\n  greet\n}\n",
            ),
        ],
        name: "greet",
        loaded: "sh/lib/common.sh",
        other: "sh/other/alt.sh",
        caller: "sh/run.sh",
        call: "  greet",
    },
    LoadCase {
        files: &[
            ("sh/lib/common.sh", "greet() {\n  echo lib\n}\n"),
            ("sh/other/alt.sh", "greet() {\n  echo other\n}\n"),
            (
                "sh/run.sh",
                "source ./lib/common.sh\n\nrun() {\n  greet\n}\n",
            ),
            (
                "sh/main.sh",
                "source ./run.sh\nsource ./other/alt.sh\nrun\n",
            ),
        ],
        name: "greet",
        loaded: "sh/lib/common.sh",
        other: "sh/other/alt.sh",
        caller: "sh/run.sh",
        call: "  greet",
    },
    LoadCase {
        files: &[
            (
                "ps/Modules/Common/Common.psm1",
                "function Get-Thing {\n  1\n}\n",
            ),
            ("ps/other/Alt.ps1", "function Get-Thing {\n  2\n}\n"),
            (
                "ps/run.ps1",
                "Import-Module Common\n\nfunction Invoke-Run {\n  Get-Thing\n}\n",
            ),
        ],
        name: "Get-Thing",
        loaded: "ps/Modules/Common/Common.psm1",
        other: "ps/other/Alt.ps1",
        caller: "ps/run.ps1",
        call: "  Get-Thing",
    },
    LoadCase {
        files: &[
            ("php/lib/x.php", "<?php\nfunction xhelper() { return 1; }\n"),
            (
                "php/other/y.php",
                "<?php\nfunction xhelper() { return 2; }\n",
            ),
            (
                "php/main.php",
                "<?php\nrequire $base . '/lib/x.php';\n\nfunction run() {\n  return xhelper();\n}\n",
            ),
        ],
        name: "xhelper",
        loaded: "php/lib/x.php",
        other: "php/other/y.php",
        caller: "php/main.php",
        call: "xhelper();",
    },
    LoadCase {
        files: &[
            ("pkg/DESCRIPTION", "Package: pkg\nVersion: 0.1\n"),
            ("pkg/R/helpers.R", "helper <- function() 1\n"),
            ("pkg/R/other.R", "helper <- function() 2\n"),
            (
                "pkg/R/main.R",
                "source(\"pkg/R/helpers.R\")\n\nrun <- function() {\n  helper()\n}\n",
            ),
        ],
        name: "helper",
        loaded: "pkg/R/helpers.R",
        other: "pkg/R/other.R",
        caller: "pkg/R/main.R",
        call: "  helper()",
    },
    LoadCase {
        files: &[
            ("rb/lib/helper.rb", "def helper_fn\n  1\nend\n"),
            ("rb/other/alt.rb", "def helper_fn\n  2\nend\n"),
            (
                "rb/models/thing.rb",
                "class Thing\n  def helper_fn\n    3\n  end\nend\n",
            ),
            (
                "rb/main.rb",
                "require_relative \"lib/helper\"\n\ndef run\n  helper_fn\nend\n",
            ),
        ],
        name: "helper_fn",
        loaded: "rb/lib/helper.rb",
        other: "rb/other/alt.rb",
        caller: "rb/main.rb",
        call: "  helper_fn",
    },
];

#[test]
fn a_load_that_the_index_cannot_follow_or_a_caller_that_another_file_loads_decides_nothing() {
    for case in UNDECIDED_LOADS {
        let repo = scanned_repo(case.files);
        let line = line_of(&repo, case.caller, case.call);

        for target in [case.loaded, case.other] {
            let report = report(&repo, case.name, target);
            let site = site_at(&report, case.caller, line);
            assert_eq!(
                site.label,
                SiteLabel::Unresolved,
                "{} from {target}: {}\n{}",
                case.caller,
                site.basis,
                dump(&report)
            );
        }
    }
}

#[test]
fn a_bash_call_above_the_source_line_does_not_see_the_loaded_file() {
    let repo = scanned_repo(&[
        ("sh/lib/common.sh", "greet() {\n  echo lib\n}\n"),
        ("sh/other/alt.sh", "greet() {\n  echo other\n}\n"),
        ("sh/top.sh", "greet\nsource ./lib/common.sh\ngreet\n"),
    ]);
    let report = report(&repo, "greet", "sh/other/alt.sh");

    assert_site(
        &report,
        "sh/top.sh",
        1,
        SiteLabel::Unresolved,
        "2 definitions",
    );
    assert_site(
        &report,
        "sh/top.sh",
        3,
        SiteLabel::Excluded,
        "the file loads `greet` from `sh/lib/common.sh`",
    );
}

const C_INCLUDES: &[(&str, &str)] = &[
    ("c/util.h", "int add(int a, int b);\n"),
    (
        "c/util.c",
        "#include \"util.h\"\n\nint add(int a, int b) { return a + b; }\n",
    ),
    ("c/alt.c", "int add(int a, int b) { return a - b; }\n"),
    (
        "c/other.c",
        "static int add(int a, int b) { return a * b; }\n\nint use(void) { return add(1, 2); }\n",
    ),
    (
        "c/main.c",
        "#include <stdio.h>\n#include \"util.h\"\n\nint main(void) {\n  return add(1, 2);\n}\n",
    ),
];

#[test]
fn a_c_include_proves_the_header_declaration_and_leaves_linking_to_pick_the_definition() {
    let repo = scanned_repo(C_INCLUDES);
    let call = line_of(&repo, "c/main.c", "return add");

    assert_site(
        &report(&repo, "add", "c/util.h"),
        "c/main.c",
        call,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "add", "c/util.c"),
        "c/main.c",
        call,
        SiteLabel::Likely,
        "the included `c/util.h` declares it",
    );
    assert_eq!(
        site_at(&report(&repo, "add", "c/alt.c"), "c/main.c", call).label,
        SiteLabel::Unresolved
    );
    let other = report(&repo, "add", "c/other.c");
    assert_site(
        &other,
        "c/main.c",
        call,
        SiteLabel::Excluded,
        "`add` is static in `c/other.c`",
    );
    assert_site(
        &other,
        "c/other.c",
        line_of(&repo, "c/other.c", "return add"),
        SiteLabel::Proven,
        "same file",
    );
}

#[test]
fn a_cpp_angle_include_of_a_project_header_proves_its_declaration_and_a_static_inline_one() {
    let repo = scanned_repo(&[
        (
            "include/proj/util.hpp",
            "int add(int a, int b);\n\nstatic inline int square(int a) { return a * a; }\n",
        ),
        (
            "src/util.cpp",
            "#include <proj/util.hpp>\n\nint add(int a, int b) { return a + b; }\n",
        ),
        (
            "src/other.cpp",
            "static int square(int a) { return a; }\n\nint use() { return square(2); }\n",
        ),
        (
            "src/main.cpp",
            "#include <vector>\n#include <proj/util.hpp>\n\nint main() {\n  return add(1, 2) + square(3);\n}\n",
        ),
    ]);
    let call = line_of(&repo, "src/main.cpp", "return add");

    assert_site(
        &report(&repo, "add", "include/proj/util.hpp"),
        "src/main.cpp",
        call,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "add", "src/util.cpp"),
        "src/main.cpp",
        call,
        SiteLabel::Likely,
        "the included `include/proj/util.hpp` declares it",
    );
    assert_site(
        &report(&repo, "square", "include/proj/util.hpp"),
        "src/main.cpp",
        call,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "square", "src/other.cpp"),
        "src/main.cpp",
        call,
        SiteLabel::Excluded,
        "`square` is static in `src/other.cpp`",
    );
}

const DART_PREFIXES: &[(&str, &str)] = &[
    ("dart/pubspec.yaml", "name: blog\n"),
    ("dart/lib/util.dart", "int shout() => 1;\n"),
    ("dart/lib/other.dart", "int shout() => 2;\n"),
    ("dart/lib/api.dart", "export 'src/impl.dart';\n"),
    ("dart/lib/src/impl.dart", "int whisper() => 1;\n"),
    ("dart/lib/quiet.dart", "int whisper() => 2;\n"),
    (
        "dart/bin/main.dart",
        "import 'package:blog/util.dart' as u;\nimport '../lib/util.dart' as s;\nimport 'package:blog/api.dart' as api;\nimport 'package:blog/other.dart' as o show other;\nimport 'package:http/http.dart' as http;\n\nvoid main() {\n  u.shout();\n  s.shout();\n  api.whisper();\n  o.shout();\n  http.shout();\n}\n",
    ),
];

#[test]
fn a_dart_import_prefix_reaches_only_the_library_it_names_and_the_files_that_library_exports() {
    let repo = scanned_repo(DART_PREFIXES);
    let main = "dart/bin/main.dart";
    let util = report(&repo, "shout", "dart/lib/util.dart");
    let other = report(&repo, "shout", "dart/lib/other.dart");

    assert_site(
        &util,
        main,
        line_of(&repo, main, "u.shout"),
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &util,
        main,
        line_of(&repo, main, "s.shout"),
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &other,
        main,
        line_of(&repo, main, "u.shout"),
        SiteLabel::Excluded,
        "receiver `u` is imported from `package:blog/util.dart`",
    );
    assert_site(
        &other,
        main,
        line_of(&repo, main, "o.shout"),
        SiteLabel::Excluded,
        "receiver `o` is imported from `package:blog/other.dart`",
    );
    assert_site(
        &util,
        main,
        line_of(&repo, main, "http.shout"),
        SiteLabel::Excluded,
        "receiver `http` is imported from `package:http/http.dart`",
    );
    assert_site(
        &report(&repo, "whisper", "dart/lib/src/impl.dart"),
        main,
        line_of(&repo, main, "api.whisper"),
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "whisper", "dart/lib/quiet.dart"),
        main,
        line_of(&repo, main, "api.whisper"),
        SiteLabel::Excluded,
        "receiver `api` is imported from `package:blog/api.dart`",
    );
}

#[test]
fn a_gdscript_preload_constant_reaches_the_class_of_its_file_and_that_class_s_bases() {
    let repo = scanned_repo(&[
        ("game/project.godot", "[application]\n"),
        (
            "game/scripts/base.gd",
            "class_name BaseHelper\nextends Node\n\nstatic func build():\n\treturn 0\n",
        ),
        (
            "game/scripts/helper.gd",
            "extends BaseHelper\n\nstatic func make():\n\treturn 1\n",
        ),
        (
            "game/scripts/other.gd",
            "extends Node\n\nstatic func make():\n\treturn 2\n\nstatic func build():\n\treturn 3\n",
        ),
        (
            "game/scripts/main.gd",
            "extends Node\n\nconst Helper = preload(\"res://scripts/helper.gd\")\n\nfunc _ready():\n\tHelper.make()\n\tHelper.build()\n",
        ),
    ]);
    let main = "game/scripts/main.gd";
    let make = line_of(&repo, main, "Helper.make");
    let build = line_of(&repo, main, "Helper.build");
    let loaded = "receiver `Helper` is loaded from `res://scripts/helper.gd`";

    assert_site(
        &report(&repo, "make", "game/scripts/helper.gd"),
        main,
        make,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "make", "game/scripts/other.gd"),
        main,
        make,
        SiteLabel::Excluded,
        loaded,
    );
    assert_site(
        &report(&repo, "build", "game/scripts/base.gd"),
        main,
        build,
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &report(&repo, "build", "game/scripts/other.gd"),
        main,
        build,
        SiteLabel::Excluded,
        loaded,
    );
}

#[test]
fn a_cpp_namespace_alias_of_the_standard_library_is_outside_and_one_of_the_project_is_not() {
    let repo = scanned_repo(&[
        (
            "src/fs.cpp",
            "bool exists(const char* p) { return p != nullptr; }\n",
        ),
        (
            "src/detail.cpp",
            "namespace mylib {\nnamespace detail {\nint run() { return 1; }\n}\n}\n",
        ),
        (
            "src/main.cpp",
            "namespace fs = std::filesystem;\nnamespace m = mylib::detail;\n\nint main() {\n  fs::exists(\"x\");\n  return m::run();\n}\n",
        ),
    ]);

    assert_site(
        &report(&repo, "exists", "src/fs.cpp"),
        "src/main.cpp",
        line_of(&repo, "src/main.cpp", "fs::exists"),
        SiteLabel::Excluded,
        "receiver `fs` is imported from `std::filesystem`",
    );
    assert_not_excluded(
        &report(&repo, "run", "src/detail.cpp"),
        "src/main.cpp",
        line_of(&repo, "src/main.cpp", "m::run"),
    );
}

#[test]
fn an_r_package_namespace_reaches_its_own_r_folder_and_any_other_package_is_outside() {
    let repo = scanned_repo(&[
        ("mypkg/DESCRIPTION", "Package: mypkg\nVersion: 0.1.0\n"),
        ("mypkg/R/helper.R", "helper <- function(x) x\n"),
        ("scripts/filter.R", "filter <- function(x) x\n"),
        ("scripts/helper.R", "helper <- function(x) x + 1\n"),
        (
            "mypkg/tests/testthat/test-helper.R",
            "library(dplyr)\n\ntest_that(\"helper\", {\n  dplyr::filter(1)\n  mypkg::helper(1)\n})\n",
        ),
    ]);
    let test = "mypkg/tests/testthat/test-helper.R";
    let helper = line_of(&repo, test, "mypkg::helper");

    assert_site(
        &report(&repo, "filter", "scripts/filter.R"),
        test,
        line_of(&repo, test, "dplyr::filter"),
        SiteLabel::Excluded,
        "receiver `dplyr` is imported from `dplyr`",
    );
    assert_site(
        &report(&repo, "helper", "mypkg/R/helper.R"),
        test,
        helper,
        SiteLabel::Proven,
        "qualified path",
    );
    assert_site(
        &report(&repo, "helper", "scripts/helper.R"),
        test,
        helper,
        SiteLabel::Excluded,
        "package `mypkg` holds only `mypkg/R/`",
    );
}

#[test]
fn a_ruby_require_binds_no_receiver_of_its_name() {
    let repo = scanned_repo(&[
        (
            "rb/store.rb",
            "class Store\n  def dig(key)\n    key\n  end\nend\n",
        ),
        (
            "rb/main.rb",
            "require \"json\"\n\ndef run(json)\n  json.dig(:a)\nend\n",
        ),
    ]);

    assert_not_excluded(
        &report(&repo, "dig", "rb/store.rb"),
        "rb/main.rb",
        line_of(&repo, "rb/main.rb", "json.dig"),
    );
}

#[test]
fn a_java_star_import_binds_no_receiver_of_its_last_name() {
    let repo = scanned_repo(&[
        (
            "java/p/Worker.java",
            "package p;\n\npublic class Worker {\n    public int run() {\n        return 1;\n    }\n}\n",
        ),
        (
            "java/p/Main.java",
            "package p;\n\nimport java.util.*;\n\nclass Main {\n    int go(Object o) {\n        var util = Factory.make(o);\n        return util.run();\n    }\n}\n",
        ),
    ]);

    assert_not_excluded(
        &report(&repo, "run", "java/p/Worker.java"),
        "java/p/Main.java",
        line_of(&repo, "java/p/Main.java", "util.run"),
    );
}

#[test]
fn a_bare_dart_call_reaches_its_own_library_first_and_then_only_what_its_imports_bring_in() {
    let repo = scanned_repo(&[
        ("dart/pubspec.yaml", "name: blog\n"),
        ("dart/lib/util.dart", "int shout() => 1;\n"),
        ("dart/lib/other.dart", "int shout() => 2;\n"),
        (
            "dart/lib/lib.dart",
            "part 'part.dart';\n\nint run() {\n  return shout();\n}\n",
        ),
        (
            "dart/lib/part.dart",
            "part of 'lib.dart';\n\nint shout() => 3;\n",
        ),
        (
            "dart/bin/main.dart",
            "import 'package:blog/util.dart';\n\nvoid main() {\n  shout();\n}\n",
        ),
        (
            "dart/bin/hidden.dart",
            "import 'package:blog/util.dart' hide shout;\nimport 'package:blog/other.dart';\n\nvoid main() {\n  shout();\n}\n",
        ),
        (
            "dart/bin/outside.dart",
            "import 'package:http/http.dart';\n\nvoid main() {\n  shout();\n}\n",
        ),
        (
            "dart/bin/conditional.dart",
            "import 'package:blog/util.dart' if (dart.library.io) 'package:blog/other.dart';\n\nvoid main() {\n  shout();\n}\n",
        ),
    ]);
    let util = report(&repo, "shout", "dart/lib/util.dart");
    let other = report(&repo, "shout", "dart/lib/other.dart");
    let part = report(&repo, "shout", "dart/lib/part.dart");
    let line = |path: &str| line_of(&repo, path, "shout()");

    assert_site(
        &util,
        "dart/bin/main.dart",
        line("dart/bin/main.dart"),
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &other,
        "dart/bin/main.dart",
        line("dart/bin/main.dart"),
        SiteLabel::Excluded,
        "the file imports `shout` from `dart/lib/util.dart`",
    );
    assert_site(
        &part,
        "dart/lib/lib.dart",
        line("dart/lib/lib.dart"),
        SiteLabel::Proven,
        "same library",
    );
    assert_site(
        &util,
        "dart/lib/lib.dart",
        line("dart/lib/lib.dart"),
        SiteLabel::Excluded,
        "the library defines `shout` in `dart/lib/part.dart`",
    );
    assert_site(
        &other,
        "dart/bin/hidden.dart",
        line("dart/bin/hidden.dart"),
        SiteLabel::Proven,
        "import",
    );
    assert_site(
        &util,
        "dart/bin/hidden.dart",
        line("dart/bin/hidden.dart"),
        SiteLabel::Excluded,
        "the file imports `shout` from `dart/lib/other.dart`",
    );
    assert_site(
        &util,
        "dart/bin/outside.dart",
        line("dart/bin/outside.dart"),
        SiteLabel::Excluded,
        "no import of the file reaches `dart/lib/util.dart`",
    );
    assert_not_excluded(
        &util,
        "dart/bin/conditional.dart",
        line("dart/bin/conditional.dart"),
    );
    assert_not_excluded(
        &other,
        "dart/bin/conditional.dart",
        line("dart/bin/conditional.dart"),
    );
    assert_ne!(
        site_at(
            &util,
            "dart/bin/conditional.dart",
            line("dart/bin/conditional.dart")
        )
        .label,
        SiteLabel::Proven,
        "{}",
        dump(&util)
    );
}

#[test]
fn a_bare_dart_call_that_a_class_method_of_its_name_could_take_decides_nothing() {
    let repo = scanned_repo(&[
        ("dart/pubspec.yaml", "name: blog\n"),
        ("dart/lib/util.dart", "int shout() => 1;\n"),
        (
            "dart/lib/speaker.dart",
            "class Speaker {\n  int shout() => 3;\n}\n",
        ),
        (
            "dart/bin/main.dart",
            "import 'package:blog/util.dart';\nimport 'package:blog/speaker.dart';\n\nclass Loud extends Speaker {\n  int run() {\n    return shout();\n  }\n}\n",
        ),
    ]);
    let line = line_of(&repo, "dart/bin/main.dart", "shout()");
    let method = report(&repo, "shout", "dart/lib/speaker.dart");
    let function = report(&repo, "shout", "dart/lib/util.dart");
    assert_not_excluded(&method, "dart/bin/main.dart", line);
    assert_ne!(
        site_at(&function, "dart/bin/main.dart", line).label,
        SiteLabel::Proven,
        "{}",
        dump(&function)
    );
}
