use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::models::{ContextSlice, FileFact, ReferenceSite, Symbol};

/// Format progressive disclosure file skeleton with implementation bodies stripped.
pub fn format_file_skeleton(file_path: &str, symbols: &[Symbol], line_count: Option<usize>) -> String {
    let mut out = String::new();
    let lines_str = match line_count {
        Some(c) => format!(" (Lines 1-{c})"),
        None => String::new(),
    };
    out.push_str(&format!("// File: {file_path}{lines_str}\n\n"));

    if symbols.is_empty() {
        out.push_str("// No exported symbols indexed.\n");
        return out;
    }

    // Group symbols by parent to render hierarchically
    let mut children_map: HashMap<Option<String>, Vec<&Symbol>> = HashMap::new();
    for s in symbols {
        children_map
            .entry(s.parent_symbol_id.clone())
            .or_default()
            .push(s);
    }

    // Render top-level symbols and their children
    if let Some(roots) = children_map.get(&None) {
        for root in roots {
            render_symbol_skeleton(&mut out, root, &children_map, 0);
        }
    } else {
        // If parent relationships are missing or flat, render all sorted by line
        for s in symbols {
            render_symbol_skeleton(&mut out, s, &children_map, 0);
        }
    }

    out
}

fn is_container_kind(kind: &str) -> bool {
    matches!(
        kind,
        "struct" | "class" | "trait" | "interface" | "enum" | "impl" | "module" | "namespace"
    )
}

fn is_skippable_kind(kind: &str) -> bool {
    matches!(kind, "variable" | "parameter" | "import")
}

fn render_symbol_skeleton(
    out: &mut String,
    sym: &Symbol,
    children_map: &HashMap<Option<String>, Vec<&Symbol>>,
    indent_level: usize,
) {
    if is_skippable_kind(&sym.kind) {
        return;
    }

    let indent = "    ".repeat(indent_level);

    // Doc comment
    if let Some(ref doc) = sym.doc_comment {
        for line in doc.lines() {
            out.push_str(&format!("{indent}/// {line}\n"));
        }
    }

    let span_str = format!("L{}-{}", sym.start_line, sym.end_line);

    // Check if this symbol is a container (class, struct, trait, enum, etc.)
    let children = children_map.get(&Some(sym.symbol_id.clone()));

    if is_container_kind(&sym.kind) && children.is_some() {
        let sig = sym.signature.as_deref().unwrap_or(&sym.name);
        out.push_str(&format!("{indent}{sig} {{\n"));
        if let Some(child_list) = children {
            for child in child_list {
                render_symbol_skeleton(out, child, children_map, indent_level + 1);
            }
        }
        out.push_str(&format!("{indent}}} // {span_str}\n\n"));
    } else {
        // Leaf symbol or function/method
        if let Some(count) = sym.hidden_body_line_count() {
            let sig = sym.signature.as_deref().unwrap_or(&sym.name);
            let b_start = sym.body_start_line.unwrap_or(sym.start_line);
            let b_end = sym.body_end_line.unwrap_or(sym.end_line);

            if count > 1 {
                out.push_str(&format!(
                    "{indent}{sig} {{ /* {count} lines hidden: L{b_start}-L{b_end} */ }}\n"
                ));
            } else {
                out.push_str(&format!("{indent}{sig}; // {span_str}\n"));
            }
        } else if let Some(ref sig) = sym.signature {
            out.push_str(&format!("{indent}{sig}; // {span_str}\n"));
        } else {
            out.push_str(&format!("{indent}{} {sym_name}; // {span_str}\n", sym.kind, sym_name = sym.name));
        }
    }
}

/// Node representing directory or file in codebase outline tree.
#[derive(Default)]
struct OutlineNode {
    files: BTreeMap<String, Vec<String>>, // file_name -> list of top symbol names with kinds
    subdirs: BTreeMap<String, OutlineNode>,
}

/// Format compact architectural outline of the repository.
pub fn format_codebase_outline(
    root_label: &str,
    files: &[FileFact],
    symbols_by_file: &HashMap<String, Vec<Symbol>>,
    max_depth: usize,
) -> String {
    let mut root_node = OutlineNode::default();

    for file in files {
        let path = Path::new(&file.path);
        let components: Vec<&str> = path
            .components()
            .map(|c| c.as_os_str().to_str().unwrap_or(""))
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            continue;
        }

        let mut curr = &mut root_node;
        let depth = components.len();

        for (i, comp) in components.iter().enumerate() {
            if i == depth - 1 {
                // Leaf file
                let mut sym_tags = Vec::new();
                if let Some(syms) = symbols_by_file.get(&file.path) {
                    for s in syms.iter().take(5) {
                        sym_tags.push(format!("{} {}", s.kind, s.name));
                    }
                    if syms.len() > 5 {
                        sym_tags.push(format!("+{} more", syms.len() - 5));
                    }
                }
                curr.files.insert(comp.to_string(), sym_tags);
            } else if i < max_depth {
                curr = curr.subdirs.entry(comp.to_string()).or_default();
            }
        }
    }

    let mut out = String::new();
    out.push_str(&format!("{root_label}/\n"));
    render_outline_tree(&mut out, &root_node, "", 0, max_depth);
    out
}

fn render_outline_tree(
    out: &mut String,
    node: &OutlineNode,
    prefix: &str,
    depth: usize,
    max_depth: usize,
) {
    if depth >= max_depth {
        return;
    }

    let total_items = node.subdirs.len() + node.files.len();
    let mut index = 0;

    // Render subdirectories
    for (name, sub) in &node.subdirs {
        index += 1;
        let is_last = index == total_items;
        let branch = if is_last { "└── " } else { "├── " };
        let next_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

        out.push_str(&format!("{prefix}{branch}{name}/\n"));
        render_outline_tree(out, sub, &next_prefix, depth + 1, max_depth);
    }

    // Render files
    for (file_name, syms) in &node.files {
        index += 1;
        let is_last = index == total_items;
        let branch = if is_last { "└── " } else { "├── " };

        let sym_suffix = if !syms.is_empty() {
            format!(" [{}]", syms.join(", "))
        } else {
            String::new()
        };

        out.push_str(&format!("{prefix}{branch}{file_name}{sym_suffix}\n"));
    }
}

/// Format surgical context bundle for a symbol.
pub fn format_context_slice(slice: &ContextSlice) -> String {
    let sym = &slice.target_symbol;
    let mut out = String::new();

    out.push_str(&format!(
        "### Target: `{}` ({}:{}-{})\n\n",
        sym.name, sym.path, sym.start_line, sym.end_line
    ));

    out.push_str(&format!("```{}\n", sym.language));
    out.push_str(&slice.target_body);
    if !slice.target_body.ends_with('\n') {
        out.push('\n');
    }
    out.push_str("```\n\n");

    if !slice.callee_signatures.is_empty() {
        out.push_str("### Dependencies (Signatures):\n");
        for callee in &slice.callee_signatures {
            out.push_str(&format!("- {callee}\n"));
        }
        out.push('\n');
    }

    if !slice.related_types.is_empty() {
        out.push_str("### Types:\n");
        for t in &slice.related_types {
            out.push_str(&format!("- {t}\n"));
        }
        out.push('\n');
    }

    if !slice.related_tests.is_empty() {
        out.push_str("### Related Tests:\n");
        for test in &slice.related_tests {
            out.push_str(&format!("- `{}` ({}:{})\n", test.name, test.path, test.start_line));
        }
        out.push('\n');
    }

    out
}

/// Format references list for callers/callees.
pub fn format_references(target_name: &str, refs: &[ReferenceSite], direction: &str) -> String {
    let mut out = String::new();
    let dir_label = if direction == "callers" { "Callers of" } else { "Callees called by" };
    out.push_str(&format!("{dir_label} `{target_name}` ({} found):\n", refs.len()));

    if refs.is_empty() {
        out.push_str("  (none)\n");
        return out;
    }

    for r in refs {
        let line_info = match r.start_line {
            Some(l) => format!(":{l}"),
            None => String::new(),
        };
        let other = if direction == "callers" { &r.from_symbol_name } else { &r.to_symbol_name };
        out.push_str(&format!("- `{other}` [{}{line_info}] (kind: {})\n", r.path, r.kind));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_file_skeleton() {
        let syms = vec![
            Symbol {
                symbol_id: "s1".into(),
                file_id: "f1".into(),
                path: "src/lib.rs".into(),
                language: "rust".into(),
                name: "do_work".into(),
                kind: "function".into(),
                signature: Some("pub fn do_work() -> Result<()>".into()),
                doc_comment: Some("Performs core work.".into()),
                visibility: Some("pub".into()),
                parent_symbol_id: None,
                start_line: 10,
                start_column: 0,
                end_line: 30,
                end_column: 1,
                start_byte: 100,
                end_byte: 300,
                body_start_line: Some(11),
                body_start_column: Some(0),
                body_end_line: Some(29),
                body_end_column: Some(1),
                body_start_byte: Some(130),
                body_end_byte: Some(298),
                body_hash: None,
                semantic_group: None,
                is_test: false,
                test_container: false,
            }
        ];

        let skeleton = format_file_skeleton("src/lib.rs", &syms, Some(35));
        assert!(skeleton.contains("/// Performs core work."));
        assert!(skeleton.contains("19 lines hidden: L11-L29"));
    }
}
