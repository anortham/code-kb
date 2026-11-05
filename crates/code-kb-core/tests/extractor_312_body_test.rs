use code_kb_core::{
    ContextSlice, Symbol,
    db::open_read_only,
    formatters::format_context_slices,
    ops::{
        SymbolSelector, file_skeleton_op, get_context_slice_selected_op,
        get_symbol_body_selected_op,
    },
    queries::load_file_symbols,
    safe_tempdir,
    sync::{find_julie_extract_binary, scan_workspace},
    workspace::Workspace,
};
use std::{fs, path::PathBuf};

struct Repo {
    _dir: tempfile::TempDir,
    workspace: Workspace,
    db: PathBuf,
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let dir = safe_tempdir();
    let root = dir.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    let workspace = Workspace::new(root);
    scan_workspace(&workspace, &db, true).expect("scan failed");
    Repo {
        _dir: dir,
        workspace,
        db,
    }
}

fn symbol_at(repo: &Repo, path: &str, name: &str, line: usize) -> Symbol {
    let conn = open_read_only(&repo.db).unwrap();
    load_file_symbols(&conn, path)
        .unwrap()
        .into_iter()
        .find(|symbol| symbol.name == name && symbol.start_line == line)
        .unwrap_or_else(|| panic!("{name} at {path}:{line} was not indexed"))
}

fn body_and_context(repo: &Repo, symbol: &Symbol) -> (String, ContextSlice, String) {
    let conn = open_read_only(&repo.db).unwrap();
    let (body_symbol, body) = get_symbol_body_selected_op(
        &repo.workspace,
        &repo.db,
        &conn,
        &SymbolSelector::Id(symbol.symbol_id.clone()),
        Some(&symbol.path),
    )
    .unwrap();
    assert_eq!(body_symbol.symbol_id, symbol.symbol_id);
    let context = get_context_slice_selected_op(
        &repo.workspace,
        &repo.db,
        &conn,
        &SymbolSelector::Id(symbol.symbol_id.clone()),
        Some(&symbol.path),
        false,
    )
    .unwrap();
    let formatted = format_context_slices(std::slice::from_ref(&context));
    (body, context, formatted)
}

fn skeleton(repo: &Repo, path: &str) -> String {
    let conn = open_read_only(&repo.db).unwrap();
    file_skeleton_op(&repo.workspace, &repo.db, &conn, path).unwrap()
}

fn assert_body_span(context: &ContextSlice, source: &str, expected: Option<&str>) {
    let symbol = &context.target_symbol;
    let actual = match (symbol.body_start_byte, symbol.body_end_byte) {
        (Some(start), Some(end)) => Some(source.get(start..end).expect("body offsets in source")),
        (None, None) => None,
        offsets => panic!("unexpected body offsets for {}: {offsets:?}", symbol.name),
    };
    assert_eq!(actual, expected, "{}", symbol.name);
    assert_eq!(
        symbol.body_start_line.is_some(),
        expected.is_some(),
        "{}",
        symbol.name
    );
    assert_eq!(
        symbol.body_start_column.is_some(),
        expected.is_some(),
        "{}",
        symbol.name
    );
    assert_eq!(
        symbol.body_end_line.is_some(),
        expected.is_some(),
        "{}",
        symbol.name
    );
    assert_eq!(
        symbol.body_end_column.is_some(),
        expected.is_some(),
        "{}",
        symbol.name
    );
    assert_eq!(
        symbol.body_hash.is_some(),
        expected.is_some(),
        "{}",
        symbol.name
    );
}

#[test]
fn c_typedef_context_assigns_the_inline_type_body_to_its_definition() {
    let source = concat!(
        "typedef struct list_node list_node_t;\n",
        "typedef struct { int x; } Foo, *FooPtr, FooArray[2], (*FooFactory)(void);\n",
        "typedef union { int i; float f; } Number;\n",
        "typedef void handler_t(int signo);\n",
        "typedef int (*visit_fn)(list_node_t *node, void *ctx);\n",
        "typedef unsigned long size_type;\n",
        "typedef int Matrix[4][4];\n",
        "struct walker { visit_fn visit; };\n",
    );
    let repo = scanned_repo(&[("src/declarators.c", source)]);
    let (foo_body, foo, foo_context) =
        body_and_context(&repo, &symbol_at(&repo, "src/declarators.c", "Foo", 2));

    assert_body_span(&foo, source, Some("{ int x; }"));
    assert!(foo_body.contains("struct { int x; }"), "{foo_body}");
    assert!(
        foo.target_body.contains("struct { int x; }"),
        "{foo_context}"
    );
    assert!(foo_context.contains("struct { int x; }"), "{foo_context}");

    for name in ["FooPtr", "FooArray", "FooFactory"] {
        let symbol = symbol_at(&repo, "src/declarators.c", name, 2);
        let (body, context, formatted) = body_and_context(&repo, &symbol);
        assert_eq!(context.target_symbol.kind, "type", "{name}");
        assert_body_span(&context, source, None);
        assert!(body.contains(name), "{name}: {body}");
        assert!(
            context.target_body.contains(name),
            "{name}: {}",
            context.target_body
        );
        assert!(formatted.contains(name), "{name}: {formatted}");
    }

    let (_, number, _) =
        body_and_context(&repo, &symbol_at(&repo, "src/declarators.c", "Number", 3));
    assert_body_span(&number, source, Some("{ int i; float f; }"));
}

#[test]
fn go_context_separates_function_literal_bodies_from_parameters_and_values() {
    let source = r#"package shapes

type Closer interface { Close() error }
type Circle struct{}
func (c *Circle) Close() error { return nil }

func run(callback func()) { callback() }

func create() {
	var empty = Circle{}
	var registry = map[string]func(){}
	var handler = func() { run(func() {}) }
	_, _, _ = empty, registry, handler
}
"#;
    let repo = scanned_repo(&[("src/shapes.go", source)]);

    for (name, line) in [("callback", 7), ("empty", 10), ("registry", 11)] {
        let symbol = symbol_at(&repo, "src/shapes.go", name, line);
        let (body, context, formatted) = body_and_context(&repo, &symbol);
        assert_body_span(&context, source, None);
        assert!(!body.is_empty(), "{name}");
        assert!(
            formatted.contains(&context.target_symbol.name),
            "{formatted}"
        );
    }

    let handler = symbol_at(&repo, "src/shapes.go", "handler", 12);
    let (body, context, formatted) = body_and_context(&repo, &handler);
    assert_body_span(&context, source, Some("{ run(func() {}) }"));
    assert!(body.contains("run(func() {})"), "{body}");
    assert!(
        context.target_body.contains("run(func() {})"),
        "{}",
        context.target_body
    );
    assert!(formatted.contains("run(func() {})"), "{formatted}");
}

#[test]
fn go_interface_method_context_does_not_claim_the_implementation_body() {
    let source = "package shapes\ntype Closer interface { Close() error }\ntype Circle struct{}\nfunc (c *Circle) Close() error { return nil }\n";
    let repo = scanned_repo(&[("src/methods.go", source)]);
    let (_, interface_method, interface_context) =
        body_and_context(&repo, &symbol_at(&repo, "src/methods.go", "Close", 2));
    let (implementation_body, implementation, implementation_context) =
        body_and_context(&repo, &symbol_at(&repo, "src/methods.go", "Close", 4));

    assert_body_span(&interface_method, source, None);
    assert!(
        interface_context.contains("Close() error"),
        "{interface_context}"
    );
    assert_body_span(&implementation, source, Some("{ return nil }"));
    assert!(
        implementation_body.contains("{ return nil }"),
        "{implementation_body}"
    );
    assert!(
        implementation_context.contains("{ return nil }"),
        "{implementation_context}"
    );
}

#[test]
fn swift_function_type_parameter_context_has_only_the_callable_body() {
    let source = "func matching(_ p: (Int) -> Bool) { _ = p(1) }\n";
    let repo = scanned_repo(&[("Sources/Matching.swift", source)]);
    let (_, parameter, parameter_context) =
        body_and_context(&repo, &symbol_at(&repo, "Sources/Matching.swift", "p", 1));
    let (function_body, function, function_context) = body_and_context(
        &repo,
        &symbol_at(&repo, "Sources/Matching.swift", "matching", 1),
    );

    assert_body_span(&parameter, source, None);
    assert!(
        parameter_context.contains("Int) -> Bool"),
        "{parameter_context}"
    );
    assert_body_span(&function, source, Some("{ _ = p(1) }"));
    assert!(
        function_body.contains("func matching(_ p: (Int) -> Bool)"),
        "{function_body}"
    );
    assert!(
        function_context.contains("{ _ = p(1) }"),
        "{function_context}"
    );
}

#[test]
fn html_external_script_context_has_no_body_and_inline_scripts_keep_their_own() {
    let source = concat!(
        "<script src=\"external.js\">inlineText()</script>\n",
        "<script src=\"external-multiline.js\">\n",
        "function fallback() {\n",
        "  return 1;\n",
        "}\n",
        "</script>\n",
        "<script type=\"text/template\">template body</script>\n",
        "<script>\n",
        "function choose(value) {\n",
        "  if (value) return value;\n",
        "  return null;\n",
        "}\n",
        "</script>\n",
    );
    let repo = scanned_repo(&[("web/page.html", source)]);
    let (external_body, external, external_context) =
        body_and_context(&repo, &symbol_at(&repo, "web/page.html", "script", 1));
    let (_, template, template_context) =
        body_and_context(&repo, &symbol_at(&repo, "web/page.html", "script", 7));
    let (_, external_multiline, external_multiline_context) =
        body_and_context(&repo, &symbol_at(&repo, "web/page.html", "script", 2));
    let (inline_body, inline, inline_context) =
        body_and_context(&repo, &symbol_at(&repo, "web/page.html", "choose", 9));

    assert_eq!(external.target_symbol.kind, "import");
    assert_body_span(&external, source, None);
    assert!(external_body.contains("external.js"), "{external_body}");
    assert!(
        external_context.contains("external.js"),
        "{external_context}"
    );
    assert_body_span(&external_multiline, source, None);
    assert!(external_multiline_context.contains("external-multiline.js"));
    assert_body_span(&template, source, Some("template body"));
    assert!(
        template_context.contains("template body"),
        "{template_context}"
    );
    assert_body_span(
        &inline,
        source,
        Some("{\n  if (value) return value;\n  return null;\n}"),
    );
    assert!(
        inline_body.contains("function choose(value)"),
        "{inline_body}"
    );
    assert!(
        inline_context.contains("function choose(value)"),
        "{inline_context}"
    );

    let skeleton = skeleton(&repo, "web/page.html");
    let inline_row = skeleton
        .lines()
        .find(|line| line.contains("function choose(value)"))
        .unwrap_or_else(|| panic!("{skeleton}"));
    assert_eq!(
        inline_row,
        "function choose(value) { /* 4 lines hidden: L9-L12 */ }"
    );
    assert!(!skeleton.contains("fallback"), "{skeleton}");
    assert!(!skeleton.contains("return value"), "{skeleton}");
}
