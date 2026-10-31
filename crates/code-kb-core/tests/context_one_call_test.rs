use code_kb_core::{
    ContextSlice,
    db::open_read_only,
    formatters::format_context_slices,
    ops::{SymbolSelector, get_context_slice_selected_op},
    queries::get_symbol_by_name,
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use std::{fs, path::PathBuf};

fn scanned_repo(files: &[(&str, &str)]) -> (tempfile::TempDir, Workspace, PathBuf) {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp_dir = safe_tempdir();
    let root = temp_dir.path().to_path_buf();
    for (path, content) in files {
        let full_path = root.join(path);
        fs::create_dir_all(full_path.parent().unwrap()).unwrap();
        fs::write(full_path, content).unwrap();
    }
    let workspace = Workspace::new(root.clone());
    let db_path = root.join(".code-kb").join("artifact.db");
    scan_workspace(&workspace, &db_path, true).expect("scan failed");
    (temp_dir, workspace, db_path)
}

fn context_of(files: &[(&str, &str)], path: &str, name: &str) -> ContextSlice {
    let (_temp, workspace, db_path) = scanned_repo(files);
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

const PYTHON_PACKAGE: &[(&str, &str)] = &[
    ("pyproject.toml", "[project]\nname = \"app\"\n"),
    ("src/app/__init__.py", ""),
    (
        "src/app/util.py",
        "def normalize(value: str) -> str:\n    return value.strip()\n\n\ndef lonely(value):\n    return value\n",
    ),
    (
        "src/app/service.py",
        "from app.util import normalize\n\n\ndef handle(raw):\n    return normalize(raw)\n",
    ),
    (
        "tests/test_util.py",
        "from app.util import normalize\n\n\ndef test_normalize():\n    assert normalize(\" a \") == \"a\"\n",
    ),
];

#[test]
fn a_context_with_cross_file_callers_lists_the_summary_and_run_commands() {
    let slice = context_of(PYTHON_PACKAGE, "src/app/util.py", "normalize");

    let callers = slice.callers.as_ref().expect("the callers report ran");
    assert!(
        callers.counts.proven + callers.counts.likely >= 2,
        "{callers:#?}"
    );
    assert!(
        callers
            .sites
            .iter()
            .any(|site| site.site.from_symbol_name == "handle"
                && site.site.path == "src/app/service.py"),
        "{callers:#?}"
    );
    let test = slice
        .tests
        .iter()
        .find(|test| test.name == "test_normalize")
        .unwrap_or_else(|| panic!("{:#?}", slice.tests));
    assert!(
        test.command
            .as_deref()
            .is_some_and(|command| command.contains("pytest")),
        "{test:#?}"
    );
    assert!(
        slice.open_ends.iter().any(|end| end.name == "handle"),
        "{:#?}",
        slice.open_ends
    );

    let text = format_context_slices(std::slice::from_ref(&slice));
    let order = [
        "### Target: `normalize`",
        "```python",
        "### Callers\nSites: ",
        "### Tests to run",
        "### Open Ends",
    ];
    let positions: Vec<usize> = order
        .iter()
        .map(|heading| {
            text.find(heading)
                .unwrap_or_else(|| panic!("{heading} missing:\n{text}"))
        })
        .collect();
    assert!(positions.windows(2).all(|pair| pair[0] < pair[1]), "{text}");
    assert!(text.contains("- `handle` [src/app/service.py:5]"), "{text}");
    assert!(
        text.contains("- `test_normalize` [tests/test_util.py:4] run: pytest"),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "find_references(symbol_id=\"{id}\")",
            id = slice.target_symbol.symbol_id
        )) && text.contains(&format!(
            "blast_radius(symbol_id=\"{id}\")",
            id = slice.target_symbol.symbol_id
        )),
        "{text}"
    );
    assert!(!text.contains("### Related Tests"), "{text}");
}

#[test]
fn a_context_with_no_caller_says_so_in_one_line() {
    let slice = context_of(PYTHON_PACKAGE, "src/app/util.py", "lonely");

    let text = format_context_slices(std::slice::from_ref(&slice));

    assert!(
        text.contains("### Callers\nNo indexed code calls or uses `lonely`.\nNot visible to this index: calls through getattr() or names built at runtime.\n\n"),
        "{text}"
    );
    assert!(!text.contains("Sites: "), "{text}");
    assert!(!text.contains("### Open Ends"), "{text}");
}

const RUST_BUILDER: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[package]\nname = \"builder\"\nversion = \"0.1.0\"\n",
    ),
    (
        "src/lib.rs",
        "pub struct Builder {\n    size: usize,\n}\n\nimpl Builder {\n    pub fn new() -> Self {\n        Builder { size: 1 }\n    }\n}\n\npub fn make() -> Builder {\n    Builder::new()\n}\n",
    ),
];

#[test]
fn a_context_prints_the_return_type_that_its_signature_does_not_spell() {
    let slice = context_of(RUST_BUILDER, "src/lib.rs", "new");

    assert_eq!(slice.return_type.as_deref(), Some("Builder"), "{slice:#?}");
    let text = format_context_slices(std::slice::from_ref(&slice));
    assert!(text.contains("Returns: `Builder`\n"), "{text}");
}

#[test]
fn a_context_leaves_out_a_return_type_that_its_signature_spells_and_types_each_callee() {
    let slice = context_of(RUST_BUILDER, "src/lib.rs", "make");

    assert_eq!(slice.return_type, None, "{slice:#?}");
    assert!(
        slice
            .callee_signatures
            .iter()
            .any(|callee| callee.contains("fn new() -> Self; Returns: `Builder` (src/lib.rs:6)")),
        "{:#?}",
        slice.callee_signatures
    );
}
