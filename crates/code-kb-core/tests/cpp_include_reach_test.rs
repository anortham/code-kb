use code_kb_core::{
    BlastRadiusResult, ReferenceReport, SiteLabel, Workspace, blast_radius_op,
    find_julie_extract_binary, format_blast_radius, open_read_only, reference_report, safe_tempdir,
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

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path))
        .unwrap_or_else(|error| panic!("{name} in {path}: {error}"));
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn walk(repo: &Repo, symbol: &str, path: &str) -> BlastRadiusResult {
    let conn = open_read_only(&repo.db).unwrap();
    let workspace = Workspace::new(repo.root.clone());
    blast_radius_op(&workspace, &conn, Some(symbol), Some(path), 5, 50).unwrap()
}

fn dump(report: &ReferenceReport) -> String {
    report
        .sites
        .iter()
        .map(|site| {
            format!(
                "{} {}:{} {} ({})\n",
                site.label.as_str(),
                site.site.path,
                site.site.start_line.unwrap_or(0),
                site.basis,
                site.site.kind
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

fn site(report: &ReferenceReport, path: &str, line: usize) -> (SiteLabel, String) {
    report
        .sites
        .iter()
        .find(|site| site.site.path == path && site.site.start_line == Some(line))
        .map(|site| (site.label, site.basis.clone()))
        .unwrap_or_else(|| panic!("no site at {path}:{line}\n{}", dump(report)))
}

const MACROS: &str = "#pragma once\n#define LIB_NAMESPACE_BEGIN namespace lib { inline namespace v2 {\n#define LIB_NAMESPACE_END } }\n";

const READER: &str = r#"#pragma once
#include <lib/macros.hpp>

LIB_NAMESPACE_BEGIN
namespace detail
{
template<typename T>
class reader
{
  public:
    explicit reader(T value) : value_(value) {}
    T get() const { return value_; }
  private:
    T value_;
};

inline int make_input(int v)
{
    return v;
}
}  // namespace detail
LIB_NAMESPACE_END
"#;

const LIB: &str = r#"#pragma once
#include <lib/reader.hpp>

LIB_NAMESPACE_BEGIN
inline int parse(int v)
{
    auto r = detail::reader<int>(detail::make_input(v));
    return r.get();
}
LIB_NAMESPACE_END
"#;

const ABI_COPY: &str = r#"#pragma once

namespace lib
{
namespace detail
{
template<typename T>
class reader
{
  public:
    explicit reader(T value) : value_(value) {}
    T get() const { return value_; }
  private:
    T value_;
};

inline int make_input(int v)
{
    return v + 1;
}
}  // namespace detail

inline int parse_v1(int v)
{
    auto r = detail::reader<int>(detail::make_input(v));
    return r.get();
}
}  // namespace lib
"#;

const UNIT: &str = r#"#include <lib/lib.hpp>

int reader_test()
{
    auto r = lib::detail::reader<int>(lib::detail::make_input(1));
    return r.get();
}

int test_parse()
{
    return lib::parse(2);
}
"#;

const ABI_USE: &str = r#"#include <lib/lib_v1.hpp>

int abi_test()
{
    auto r = lib::detail::reader<int>(lib::detail::make_input(1));
    return r.get() + lib::parse_v1(2);
}
"#;

fn nlohmann_shape(extra: &[(&'static str, &'static str)]) -> Repo {
    let mut files = vec![
        ("include/lib/macros.hpp", MACROS),
        ("include/lib/reader.hpp", READER),
        ("include/lib/lib.hpp", LIB),
        ("tests/abi/include/lib/lib_v1.hpp", ABI_COPY),
        ("tests/src/unit-reader.cpp", UNIT),
        ("tests/abi/use_v1.cpp", ABI_USE),
    ];
    files.extend_from_slice(extra);
    scanned_repo(&files)
}

const ABI: &str = "tests/abi/include/lib/lib_v1.hpp";

#[test]
fn a_copy_header_that_no_include_reaches_never_owns_a_qualified_site() {
    let repo = nlohmann_shape(&[]);
    let copy = report(&repo, "reader", ABI);
    let text = dump(&copy);

    for (path, needle) in [
        (
            "include/lib/lib.hpp",
            "detail::reader<int>(detail::make_input",
        ),
        ("tests/src/unit-reader.cpp", "lib::detail::reader<int>("),
    ] {
        let (label, basis) = site(&copy, path, line_of(&repo, path, needle));
        assert_eq!(label, SiteLabel::Excluded, "{path}\n{text}");
        assert!(
            basis.contains("`include/lib/reader.hpp`") && basis.contains(ABI),
            "{path}: {basis}\n{text}"
        );
    }
    let (label, _) = site(
        &copy,
        "tests/abi/use_v1.cpp",
        line_of(&repo, "tests/abi/use_v1.cpp", "lib::detail::reader<int>("),
    );
    assert_ne!(label, SiteLabel::Excluded, "{text}");
}

#[test]
fn a_definition_in_a_copy_header_does_not_take_a_site_from_the_included_one() {
    let repo = nlohmann_shape(&[]);
    let main = report(&repo, "make_input", "include/lib/reader.hpp");
    let text = dump(&main);

    for (path, needle) in [
        ("include/lib/lib.hpp", "detail::make_input(v)"),
        ("tests/src/unit-reader.cpp", "lib::detail::make_input(1)"),
    ] {
        let (label, basis) = site(&main, path, line_of(&repo, path, needle));
        assert!(
            matches!(label, SiteLabel::Proven | SiteLabel::Likely),
            "{path}: {label:?} {basis}\n{text}"
        );
        assert!(!basis.contains(ABI), "{path}: {basis}\n{text}");
    }
    let (label, basis) = site(
        &main,
        "tests/abi/use_v1.cpp",
        line_of(&repo, "tests/abi/use_v1.cpp", "lib::detail::make_input(1)"),
    );
    assert_eq!(label, SiteLabel::Excluded, "{basis}\n{text}");
}

#[test]
fn the_walk_leaves_a_copy_header_out_of_the_tests_it_lists() {
    let repo = nlohmann_shape(&[]);
    let copy = walk(&repo, "reader", ABI);
    let text = format_blast_radius(&copy);
    assert!(
        copy.likely_tests
            .iter()
            .all(|test| test.name != "reader_test"),
        "{text}"
    );
    assert!(
        copy.impacted_symbols
            .iter()
            .all(|symbol| symbol.path != "include/lib/lib.hpp"),
        "{text}"
    );

    let main = walk(&repo, "make_input", "include/lib/reader.hpp");
    let text = format_blast_radius(&main);
    assert!(
        main.likely_tests
            .iter()
            .any(|test| test.name == "test_parse"),
        "{text}"
    );
    assert!(
        main.open_ends.iter().all(|end| end.name != "make_input"),
        "{text}"
    );
}

#[test]
fn a_header_included_through_another_header_still_proves_its_declaration() {
    let repo = scanned_repo(&[
        (
            "src/util/inner.hpp",
            "#pragma once\nint helper(int a);\n\nstruct Point\n{\n    int x;\n    int y;\n};\n",
        ),
        (
            "src/util/outer.hpp",
            "#pragma once\n#include \"inner.hpp\"\n",
        ),
        (
            "src/app.cpp",
            "#include \"util/outer.hpp\"\n\nint run()\n{\n    Point p = Point{1, 2};\n    return helper(p.x);\n}\n",
        ),
        (
            "src/other/inner.hpp",
            "#pragma once\nstruct Point\n{\n    int x;\n};\n",
        ),
    ]);
    let call = line_of(&repo, "src/app.cpp", "return helper");
    let declared = report(&repo, "helper", "src/util/inner.hpp");
    let (label, basis) = site(&declared, "src/app.cpp", call);
    assert_eq!(
        (label, basis.as_str()),
        (SiteLabel::Proven, "import"),
        "{}",
        dump(&declared)
    );

    let point = line_of(&repo, "src/app.cpp", "Point p");
    let included = report(&repo, "Point", "src/util/inner.hpp");
    let (label, basis) = site(&included, "src/app.cpp", point);
    assert_ne!(label, SiteLabel::Excluded, "{basis}\n{}", dump(&included));
    let other = report(&repo, "Point", "src/other/inner.hpp");
    let (label, basis) = site(&other, "src/app.cpp", point);
    assert_eq!(
        (label, basis.as_str()),
        (
            SiteLabel::Excluded,
            "this file includes `Point` from `src/util/inner.hpp`, not from `src/other/inner.hpp`"
        ),
        "{}",
        dump(&other)
    );
}

#[test]
fn a_qualified_use_of_a_class_in_an_included_header_stays_proven() {
    let repo = scanned_repo(&[
        (
            "include/shapes/widget.hpp",
            "#pragma once\nnamespace shapes {\nclass Widget {\n  public:\n    static Widget make() { return Widget(); }\n    int size() const { return 1; }\n};\n}\n",
        ),
        (
            "src/main.cpp",
            "#include <shapes/widget.hpp>\n\nint main()\n{\n    shapes::Widget w = shapes::Widget::make();\n    return w.size();\n}\n",
        ),
    ]);
    let widget = report(&repo, "make", "include/shapes/widget.hpp");
    let (label, basis) = site(
        &widget,
        "src/main.cpp",
        line_of(&repo, "src/main.cpp", "shapes::Widget::make()"),
    );
    assert_eq!(label, SiteLabel::Proven, "{basis}\n{}", dump(&widget));
}

#[test]
fn a_definition_in_the_source_file_of_an_included_header_stays_likely() {
    let repo = scanned_repo(&[
        ("src/util.hpp", "#pragma once\nint add(int a, int b);\n"),
        (
            "src/util.cpp",
            "#include \"util.hpp\"\n\nint add(int a, int b) { return a + b; }\n",
        ),
        (
            "src/main.cpp",
            "#include \"util.hpp\"\n\nint main()\n{\n    return add(1, 2);\n}\n",
        ),
    ]);
    let definition = report(&repo, "add", "src/util.cpp");
    let (label, basis) = site(
        &definition,
        "src/main.cpp",
        line_of(&repo, "src/main.cpp", "return add"),
    );
    assert_eq!(
        (label, basis.as_str()),
        (SiteLabel::Likely, "the included `src/util.hpp` declares it"),
        "{}",
        dump(&definition)
    );
}

#[test]
fn an_include_that_julie_did_not_record_keeps_the_copy_header_in_reach() {
    let repo = nlohmann_shape(&[(
        "tests/src/unit-config.cpp",
        "#include <lib/lib.hpp>\n#include LIB_CONFIG_HEADER\n\nint config_test()\n{\n    auto r = lib::detail::reader<int>(lib::detail::make_input(3));\n    return r.get();\n}\n",
    )]);
    let copy = report(&repo, "reader", ABI);
    let path = "tests/src/unit-config.cpp";
    let (label, basis) = site(
        &copy,
        path,
        line_of(&repo, path, "lib::detail::reader<int>("),
    );
    assert_ne!(label, SiteLabel::Excluded, "{basis}\n{}", dump(&copy));
}

#[test]
fn a_tag_declaration_in_reach_keeps_a_type_defined_outside_it() {
    let repo = scanned_repo(&[
        (
            "include/gadget_fwd.hpp",
            "#pragma once\nclass Gadget;\nint use_gadget(Gadget *g);\n",
        ),
        (
            "include/gadget.hpp",
            "#pragma once\nclass Gadget {\n  public:\n    int id() const { return 7; }\n};\n",
        ),
        (
            "src/gadget.cpp",
            "#include \"gadget.hpp\"\n#include \"gadget_fwd.hpp\"\n\nint use_gadget(Gadget *g) { return g->id(); }\n",
        ),
        (
            "src/main.cpp",
            "#include \"gadget_fwd.hpp\"\n\nint run(Gadget *g)\n{\n    return use_gadget(g);\n}\n",
        ),
    ]);
    let gadget = report(&repo, "Gadget", "include/gadget.hpp");
    for site in gadget
        .sites
        .iter()
        .filter(|site| site.site.path == "src/main.cpp")
    {
        assert_ne!(site.label, SiteLabel::Excluded, "{}", dump(&gadget));
    }
}

fn assert_not_excluded(report: &ReferenceReport, path: &str) {
    let in_path: Vec<_> = report
        .sites
        .iter()
        .filter(|site| site.site.path == path)
        .collect();
    assert!(!in_path.is_empty(), "no site in {path}\n{}", dump(report));
    for site in in_path {
        assert_ne!(site.label, SiteLabel::Excluded, "{path}\n{}", dump(report));
    }
}

#[test]
fn a_template_source_file_that_its_header_includes_sees_what_the_header_includes() {
    let repo = scanned_repo(&[
        (
            "include/item.h",
            "#pragma once\nstruct Item {\n    int value;\n};\n",
        ),
        (
            "include/stack.h",
            "#pragma once\n#include \"item.h\"\n\ntemplate <typename T>\nclass Stack {\npublic:\n    void push(const Item &item);\n    int size() const;\nprivate:\n    int count_ = 0;\n};\n\n#include \"stack.cpp\"\n",
        ),
        (
            "include/stack.cpp",
            "template <typename T>\nvoid Stack<T>::push(const Item &item)\n{\n    Item copy = item;\n    count_ += copy.value;\n}\n\ntemplate <typename T>\nint Stack<T>::size() const\n{\n    return count_;\n}\n",
        ),
        (
            "src/main.cpp",
            "#include \"stack.h\"\n\nint main()\n{\n    Stack<int> s;\n    Item item{1};\n    s.push(item);\n    return s.size();\n}\n",
        ),
        (
            "other/item.h",
            "#pragma once\nstruct Item {\n    long value;\n};\n",
        ),
        (
            "other/use.cpp",
            "#include \"item.h\"\n\nlong use_item()\n{\n    Item item{2};\n    return item.value;\n}\n",
        ),
    ]);
    let item = report(&repo, "Item", "include/item.h");
    assert_not_excluded(&item, "include/stack.cpp");
    for (path, needle) in [
        ("include/stack.cpp", "Item copy"),
        ("include/stack.h", "void push(const Item"),
    ] {
        let (label, basis) = site(&item, path, line_of(&repo, path, needle));
        assert_eq!(label, SiteLabel::Likely, "{path}: {basis}\n{}", dump(&item));
    }
    let (label, basis) = site(
        &item,
        "other/use.cpp",
        line_of(&repo, "other/use.cpp", "Item item{2}"),
    );
    assert_eq!(
        (label, basis.as_str()),
        (
            SiteLabel::Excluded,
            "this file includes `Item` from `other/item.h`, not from `include/item.h`"
        ),
        "{}",
        dump(&item)
    );

    let walked = walk(&repo, "Item", "include/item.h");
    let text = format_blast_radius(&walked);
    assert!(
        walked
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.path == "include/stack.cpp"),
        "{text}"
    );
}

#[test]
fn a_source_file_of_a_unity_build_sees_the_files_included_before_it() {
    let repo = scanned_repo(&[
        (
            "src/unity_a.cpp",
            "struct Widget {\n    int id;\n};\n\nstatic int widget_id(const Widget &w) { return w.id; }\n",
        ),
        (
            "src/unity_b.cpp",
            "int use_widget()\n{\n    Widget w{3};\n    return widget_id(w);\n}\n",
        ),
        (
            "src/unity.cpp",
            "#include \"unity_a.cpp\"\n#include \"unity_b.cpp\"\n",
        ),
        (
            "tools/widget.h",
            "#pragma once\nstruct Widget {\n    long id;\n};\n",
        ),
        (
            "tools/main.cpp",
            "#include \"widget.h\"\n\nlong tool_main()\n{\n    Widget w{4};\n    return w.id;\n}\n",
        ),
    ]);
    let widget = report(&repo, "Widget", "src/unity_a.cpp");
    assert_not_excluded(&widget, "src/unity_b.cpp");
    let (label, _) = site(
        &widget,
        "tools/main.cpp",
        line_of(&repo, "tools/main.cpp", "Widget w{4}"),
    );
    assert_eq!(label, SiteLabel::Excluded, "{}", dump(&widget));
}

#[test]
fn an_import_or_include_next_that_julie_did_not_record_decides_nothing() {
    let repo = scanned_repo(&[
        (
            "c/point_types.h",
            "#ifndef POINT_TYPES_H\n#define POINT_TYPES_H\n\ntypedef struct point {\n    int x;\n    int y;\n} point_t;\n\n#endif\n",
        ),
        (
            "c/point.h",
            "#ifndef POINT_H\n#define POINT_H\n\n#include \"point_types.h\"\n\n#endif\n",
        ),
        (
            "c/anon.c",
            "#import \"point.h\"\n\nint use_point(void)\n{\n    point_t q = {1, 2};\n    return q.x;\n}\n",
        ),
        (
            "c/next.c",
            "#include_next <point.h>\n\nint next_point(void)\n{\n    point_t q = {3, 4};\n    return q.y;\n}\n",
        ),
        (
            "c/plain.c",
            "#include \"other.h\"\n\nint plain_point(void)\n{\n    point_t q = {5, 6};\n    return q.x;\n}\n",
        ),
        ("c/other.h", "#pragma once\ntypedef int point_t;\n"),
    ]);
    let point = report(&repo, "point_t", "c/point_types.h");
    assert_not_excluded(&point, "c/anon.c");
    assert_not_excluded(&point, "c/next.c");
    let (label, basis) = site(
        &point,
        "c/plain.c",
        line_of(&repo, "c/plain.c", "point_t q"),
    );
    assert_eq!(
        (label, basis.as_str()),
        (
            SiteLabel::Excluded,
            "this file includes `point_t` from `c/other.h`, not from `c/point_types.h`"
        ),
        "{}",
        dump(&point)
    );
}

#[test]
fn an_include_that_more_than_eight_files_match_decides_nothing() {
    let mut files: Vec<(String, String)> = ["a", "b", "c", "d", "e", "f", "g", "h", "zz"]
        .iter()
        .map(|module| {
            (
                format!("mods/{module}/include/config.h"),
                format!(
                    "#pragma once\nstruct {module}_cfg {{ int v; }};\ntypedef struct {module}_cfg {module}_cfg_t;\n"
                ),
            )
        })
        .collect();
    files.push((
        "src/main.c".to_string(),
        "#include <config.h>\n\nint read_cfg(void)\n{\n    zz_cfg_t d = {1};\n    return d.v;\n}\n"
            .to_string(),
    ));
    files.push((
        "lib/one/include/settings.h".to_string(),
        "#pragma once\ntypedef struct { int v; } settings_t;\n".to_string(),
    ));
    files.push((
        "lib/two/include/settings.h".to_string(),
        "#pragma once\ntypedef struct { long v; } settings_t;\n".to_string(),
    ));
    files.push((
        "lib/two/local.h".to_string(),
        "#pragma once\ntypedef struct { short v; } settings_t;\n".to_string(),
    ));
    files.push((
        "src/settings.c".to_string(),
        "#include <settings.h>\n\nint read_settings(void)\n{\n    settings_t s = {1};\n    return (int)s.v;\n}\n"
            .to_string(),
    ));
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let repo = scanned_repo(&borrowed);
    let config = report(&repo, "zz_cfg_t", "mods/zz/include/config.h");
    assert_not_excluded(&config, "src/main.c");

    let local = report(&repo, "settings_t", "lib/two/local.h");
    let (label, basis) = site(
        &local,
        "src/settings.c",
        line_of(&repo, "src/settings.c", "settings_t s"),
    );
    assert_eq!(label, SiteLabel::Excluded, "{basis}\n{}", dump(&local));
}

#[test]
fn a_call_qualified_by_a_namespace_in_reach_does_not_step_to_a_global_of_its_name() {
    let repo = scanned_repo(&[
        (
            "include/util.h",
            "#pragma once\n\nnamespace util {\ninline int helper(int v)\n{\n    return v + 1;\n}\n}  // namespace util\n",
        ),
        (
            "include/legacy.h",
            "#pragma once\n\ninline int helper(int v)\n{\n    return v - 1;\n}\n",
        ),
        (
            "src/compute.cpp",
            "#include \"legacy.h\"\n#include \"util.h\"\n\nint compute(int v)\n{\n    return util::helper(v);\n}\n\nint compute_legacy(int v)\n{\n    return helper(v);\n}\n",
        ),
    ]);
    let legacy = walk(&repo, "helper", "include/legacy.h");
    let text = format_blast_radius(&legacy);
    assert!(
        legacy
            .impacted_symbols
            .iter()
            .all(|symbol| symbol.name != "compute"),
        "{text}"
    );
    assert!(
        legacy
            .impacted_symbols
            .iter()
            .any(|symbol| symbol.name == "compute_legacy"),
        "{text}"
    );
    assert!(
        legacy.open_ends.iter().all(|end| end.name != "compute"),
        "{text}"
    );

    let (label, basis) = site(
        &report(&repo, "helper", "include/legacy.h"),
        "src/compute.cpp",
        line_of(&repo, "src/compute.cpp", "util::helper(v)"),
    );
    assert_eq!(label, SiteLabel::Excluded, "{basis}");
}

#[test]
fn a_header_sees_what_its_includers_include_and_an_orphan_header_decides_nothing() {
    let repo = scanned_repo(&[
        (
            "include/types.hpp",
            "#pragma once\nstruct Config\n{\n    int a;\n};\n",
        ),
        (
            "include/use.hpp",
            "#pragma once\ninline int read_config(Config c)\n{\n    return c.a;\n}\n",
        ),
        (
            "include/orphan.hpp",
            "#pragma once\ninline int orphan_config(Config c)\n{\n    return c.a;\n}\n",
        ),
        (
            "src/main.cpp",
            "#include \"types.hpp\"\n#include \"use.hpp\"\n\nint main()\n{\n    return read_config(Config{1});\n}\n",
        ),
        (
            "other/types.hpp",
            "#pragma once\nstruct Config\n{\n    long a;\n};\n",
        ),
        (
            "other/main.cpp",
            "#include \"types.hpp\"\n\nint other_main()\n{\n    Config c{2};\n    return static_cast<int>(c.a);\n}\n",
        ),
    ]);
    let config = report(&repo, "Config", "include/types.hpp");
    let text = dump(&config);
    for (path, needle) in [
        ("include/use.hpp", "read_config(Config c)"),
        ("include/orphan.hpp", "orphan_config(Config c)"),
    ] {
        let (label, basis) = site(&config, path, line_of(&repo, path, needle));
        assert_ne!(label, SiteLabel::Excluded, "{path}: {basis}\n{text}");
    }
    let (label, basis) = site(
        &config,
        "other/main.cpp",
        line_of(&repo, "other/main.cpp", "Config c{2}"),
    );
    assert_eq!(
        (label, basis.as_str()),
        (
            SiteLabel::Excluded,
            "this file includes `Config` from `other/types.hpp`, not from `include/types.hpp`"
        ),
        "{text}"
    );
}
