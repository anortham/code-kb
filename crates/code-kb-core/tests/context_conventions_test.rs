use code_kb_core::{
    ContextSlice, Convention,
    db::open_read_only,
    formatters::format_context_slices,
    ops::{SymbolSelector, get_context_slice_selected_op},
    queries::get_symbol_by_name,
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use std::fs;

fn context_of(files: &[(&str, &str)], path: &str, name: &str) -> ContextSlice {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (file, content) in files {
        let full_path = root.join(file);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(full_path, content).unwrap();
    }
    let workspace = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&workspace, &db_path, true).expect("scan failed");
    let conn = open_read_only(&db_path).unwrap();
    let target = get_symbol_by_name(&conn, name, Some(path))
        .unwrap()
        .unwrap();
    get_context_slice_selected_op(
        &workspace,
        &db_path,
        &conn,
        &SymbolSelector::Id(target.symbol_id),
        Some(path),
        false,
    )
    .unwrap()
}

fn shown(conventions: &[Convention]) -> Vec<(String, usize, usize, bool)> {
    conventions
        .iter()
        .map(|c| (c.text.clone(), c.siblings_with, c.siblings, c.target_has))
        .collect()
}

fn row(text: &str, with: usize, siblings: usize, has: bool) -> (String, usize, usize, bool) {
    (text.to_string(), with, siblings, has)
}

const CSHARP_PERIOD: &str = "namespace NodaTime\n{\n    public sealed class Period\n    {\n        [Pure] public Period Plus(Period other) => this;\n        [Pure] public Period Minus(Period other) => this;\n        [Pure] public Period Negate() => this;\n        public Period Normalize() => this;\n    }\n}\n";

#[test]
fn csharp_target_without_the_pure_attribute_that_every_sibling_carries_lacks_it() {
    let slice = context_of(
        &[("src/Period.cs", CSHARP_PERIOD)],
        "src/Period.cs",
        "Normalize",
    );

    assert_eq!(shown(&slice.conventions), vec![row("Pure", 3, 3, false)]);
    let text = format_context_slices(std::slice::from_ref(&slice));
    let tests = text.find("### Tests to run").unwrap();
    let conventions = text
        .find("### Conventions\nMarkers that at least two thirds of the other 3 `method` symbols of its owner carry:\n- `Pure`: 3 of 3 siblings; the target lacks it\n")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(tests < conventions, "{text}");
}

#[test]
fn csharp_target_with_the_shared_attribute_has_it() {
    let slice = context_of(&[("src/Period.cs", CSHARP_PERIOD)], "src/Period.cs", "Plus");

    assert_eq!(shown(&slice.conventions), vec![row("Pure", 2, 3, true)]);
}

#[test]
fn java_override_on_most_sibling_methods_is_a_convention() {
    let slice = context_of(
        &[(
            "src/Shape.java",
            "public class Shape extends Base {\n    @Override public int area() { return 1; }\n    @Override public int size() { return 2; }\n    @Override public String name() { return \"s\"; }\n    public int edges() { return 4; }\n}\n",
        )],
        "src/Shape.java",
        "edges",
    );

    assert_eq!(
        shown(&slice.conventions),
        vec![row("Override", 3, 3, false)]
    );
}

#[test]
fn python_decorators_on_sibling_methods_list_the_lacking_one_first() {
    let slice = context_of(
        &[(
            "src/app/model.py",
            "class Model:\n    @cached\n    @traced\n    def one(self):\n        return 1\n\n    @cached\n    @traced\n    def two(self):\n        return 2\n\n    @cached\n    def three(self):\n        return 3\n\n    @traced\n    def four(self):\n        return 4\n",
        )],
        "src/app/model.py",
        "four",
    );

    assert_eq!(
        shown(&slice.conventions),
        vec![row("cached", 3, 3, false), row("traced", 2, 3, true)]
    );
}

#[test]
fn rust_derive_on_top_level_structs_shows_its_carrier() {
    let slice = context_of(
        &[(
            "src/lib.rs",
            "#[derive(Debug, Clone)]\npub struct A;\n\n#[derive(Debug, Clone)]\npub struct B;\n\n#[derive(Debug)]\npub struct C;\n\npub struct Target;\n\npub fn helper() {}\n",
        )],
        "src/lib.rs",
        "Target",
    );

    assert_eq!(
        shown(&slice.conventions),
        vec![
            row("derive(Debug)", 3, 3, false),
            row("derive(Clone)", 2, 3, false)
        ]
    );
}

#[test]
fn rust_test_attribute_on_sibling_test_functions_is_a_convention() {
    let slice = context_of(
        &[(
            "tests/checks.rs",
            "#[test]\nfn first() {}\n\n#[test]\nfn second() {}\n\n#[test]\nfn third() {}\n\nfn fourth() {}\n",
        )],
        "tests/checks.rs",
        "fourth",
    );

    assert_eq!(shown(&slice.conventions), vec![row("test", 3, 3, false)]);
}

#[test]
fn typescript_decorators_on_class_members_are_conventions() {
    let slice = context_of(
        &[(
            "src/card.ts",
            "export class Card {\n  @Input() title: string;\n  @Input() body: string;\n  @Input() footer: string;\n  size: number;\n}\n",
        )],
        "src/card.ts",
        "size",
    );

    assert_eq!(shown(&slice.conventions), vec![row("Input()", 3, 3, false)]);
}

#[test]
fn a_target_with_one_sibling_gets_no_conventions() {
    let slice = context_of(
        &[(
            "src/Pair.cs",
            "public class Pair\n{\n    [Pure] public int First() => 1;\n    public int Second() => 2;\n}\n",
        )],
        "src/Pair.cs",
        "Second",
    );

    assert!(slice.conventions.is_empty(), "{:?}", slice.conventions);
    let text = format_context_slices(std::slice::from_ref(&slice));
    assert!(!text.contains("### Conventions"), "{text}");
}

#[test]
fn siblings_with_different_markers_share_no_convention() {
    let slice = context_of(
        &[(
            "src/Mixed.cs",
            "public class Mixed\n{\n    [Pure] public int A() => 1;\n    [Obsolete] public int B() => 2;\n    [DebuggerStepThrough] public int C() => 3;\n    public int D() => 4;\n}\n",
        )],
        "src/Mixed.cs",
        "D",
    );

    assert!(slice.conventions.is_empty(), "{:?}", slice.conventions);
}

#[test]
fn a_marker_on_one_third_of_the_siblings_is_no_convention_and_two_thirds_is() {
    let files = &[(
        "src/Thirds.cs",
        "public class Thirds\n{\n    [Pure] [Obsolete] public int A() => 1;\n    [Pure] public int B() => 2;\n    public int C() => 3;\n    public int D() => 4;\n}\n",
    )];

    let slice = context_of(files, "src/Thirds.cs", "D");

    assert_eq!(shown(&slice.conventions), vec![row("Pure", 2, 3, false)]);
}

#[test]
fn context_json_leaves_out_empty_conventions_and_names_each_field() {
    let with = context_of(
        &[("src/Period.cs", CSHARP_PERIOD)],
        "src/Period.cs",
        "Normalize",
    );
    let json = serde_json::to_value(&with).unwrap();
    assert_eq!(
        json["conventions"],
        serde_json::json!([{
            "annotation": "Pure",
            "text": "Pure",
            "siblings_with": 3,
            "siblings": 3,
            "target_has": false
        }])
    );

    let without = context_of(
        &[(
            "src/Pair.cs",
            "public class Pair\n{\n    public int First() => 1;\n    public int Second() => 2;\n}\n",
        )],
        "src/Pair.cs",
        "Second",
    );
    let json = serde_json::to_value(&without).unwrap();
    assert!(json.get("conventions").is_none(), "{json}");
}
