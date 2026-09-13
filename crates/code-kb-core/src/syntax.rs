use std::path::Path;
use thiserror::Error;
use tree_sitter::{Language, Parser};

#[derive(Debug, Error, PartialEq)]
pub enum SyntaxError {
    #[error("Syntax error in {0}: {1}")]
    ParseError(String, String),
}

enum SupportedGrammar {
    Rust,
    JavaScript,
    TypeScript,
    Tsx,
    Python,
    Go,
}

fn detect_grammar(file_path: &str) -> Option<SupportedGrammar> {
    let ext = Path::new(file_path).extension()?.to_str()?;
    match ext {
        "rs" => Some(SupportedGrammar::Rust),
        "js" | "mjs" | "cjs" | "jsx" => Some(SupportedGrammar::JavaScript),
        "ts" | "mts" | "cts" => Some(SupportedGrammar::TypeScript),
        "tsx" => Some(SupportedGrammar::Tsx),
        "py" | "pyi" => Some(SupportedGrammar::Python),
        "go" => Some(SupportedGrammar::Go),
        _ => None,
    }
}

fn get_language(grammar: SupportedGrammar) -> Language {
    match grammar {
        SupportedGrammar::Rust => Language::from(tree_sitter_rust::LANGUAGE),
        SupportedGrammar::JavaScript => Language::from(tree_sitter_javascript::LANGUAGE),
        SupportedGrammar::TypeScript => Language::from(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
        SupportedGrammar::Tsx => Language::from(tree_sitter_typescript::LANGUAGE_TSX),
        SupportedGrammar::Python => Language::from(tree_sitter_python::LANGUAGE),
        SupportedGrammar::Go => Language::from(tree_sitter_go::LANGUAGE),
    }
}

fn find_first_error(node: tree_sitter::Node) -> Option<(usize, usize, String)> {
    if node.is_error() {
        let start = node.start_position();
        return Some((start.row + 1, start.column + 1, "syntax error".to_string()));
    }
    if node.is_missing() {
        let start = node.start_position();
        return Some((
            start.row + 1,
            start.column + 1,
            format!("missing {}", node.kind()),
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.has_error()
            && let Some(err) = find_first_error(child)
        {
            return Some(err);
        }
    }
    None
}

/// Pre-flight validates code syntax before it is committed to disk using language-specific Tree-Sitter grammars.
pub fn validate_syntax(file_path: &str, content: &str) -> Result<(), SyntaxError> {
    let grammar = match detect_grammar(file_path) {
        Some(g) => g,
        None => return Ok(()), // Unrecognized or non-code extensions pass through
    };

    let language = get_language(grammar);
    let mut parser = Parser::new();
    parser.set_language(&language).map_err(|e| {
        SyntaxError::ParseError(
            file_path.to_string(),
            format!("failed to initialize parser: {e}"),
        )
    })?;

    let tree = parser.parse(content, None).ok_or_else(|| {
        SyntaxError::ParseError(file_path.to_string(), "failed to parse content".to_string())
    })?;

    if tree.root_node().has_error() {
        if let Some((line, col, msg)) = find_first_error(tree.root_node()) {
            return Err(SyntaxError::ParseError(
                file_path.to_string(),
                format!("{msg} at line {line}, column {col}"),
            ));
        }
        return Err(SyntaxError::ParseError(
            file_path.to_string(),
            "syntax error detected".to_string(),
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_rust_code() {
        let code = r#"
        pub fn calculate(x: i32) -> i32 {
            let s = "Hello (world) {test}";
            // Comment with unbalanced { [ (
            /* Multi-line comment ( [ { */
            x * 2
        }
        "#;
        assert!(validate_syntax("test.rs", code).is_ok());
    }

    #[test]
    fn test_rust_lifetime_and_raw_string() {
        let code = r##"
        pub fn greet(name: &'static str) -> &'static str {
            let raw = r#"hello "world""#;
            name
        }
        "##;
        assert!(
            validate_syntax("greet.rs", code).is_ok(),
            "Valid Rust with lifetime and raw string should pass"
        );
    }

    #[test]
    fn test_invalid_rust_syntax() {
        let code = "pub fn foo() { let x = ; }";
        let res = validate_syntax("test.rs", code);
        assert!(matches!(res, Err(SyntaxError::ParseError(..))));
    }

    #[test]
    fn test_valid_javascript() {
        let code = "function greet(name) { return `hello ${name}`; }";
        assert!(validate_syntax("app.js", code).is_ok());
    }

    #[test]
    fn test_invalid_javascript_syntax() {
        let code = "function f() { const x = ; }";
        assert!(
            validate_syntax("app.js", code).is_err(),
            "Invalid JavaScript should fail syntax validation"
        );
    }

    #[test]
    fn test_valid_typescript() {
        let code = "interface User { id: number; name: string; }\nexport const get = (u: User): number => u.id;";
        assert!(validate_syntax("user.ts", code).is_ok());
    }

    #[test]
    fn test_invalid_typescript() {
        let code = "interface User { id number; }";
        assert!(validate_syntax("user.ts", code).is_err());
    }

    #[test]
    fn test_valid_python() {
        let code = "def greet(name: str) -> str:\n    return f'hello {name}'\n";
        assert!(validate_syntax("script.py", code).is_ok());
    }

    #[test]
    fn test_invalid_python_syntax() {
        let code = "def foo(:\n    pass";
        assert!(
            validate_syntax("script.py", code).is_err(),
            "Invalid Python should fail syntax validation"
        );
    }

    #[test]
    fn test_valid_go() {
        let code = "package main\n\nfunc main() {\n    println(\"hello\")\n}\n";
        assert!(validate_syntax("main.go", code).is_ok());
    }

    #[test]
    fn test_invalid_go_syntax() {
        let code = "package main\n\nfunc main() {\n    x :=\n}\n";
        assert!(validate_syntax("main.go", code).is_err());
    }

    #[test]
    fn test_unrecognized_extension_passes() {
        let code = "any unparseable random content { [";
        assert!(validate_syntax("notes.txt", code).is_ok());
    }
}
