use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::models::{
    BlastRadiusResult, ContextSlice, FileFact, ReferenceSite, Symbol, SymbolSearchResult,
};

/// Format progressive disclosure file skeleton with implementation bodies stripped.
pub fn format_file_skeleton(
    file_path: &str,
    symbols: &[Symbol],
    line_count: Option<usize>,
) -> String {
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
            out.push_str(&format!(
                "{indent}{} {sym_name}; // {span_str}\n",
                sym.kind,
                sym_name = sym.name
            ));
        }
    }
}

/// Node representing directory or file in codebase outline tree.
#[derive(Default)]
pub struct OutlineNode {
    pub files: BTreeMap<String, Vec<String>>, // file_name -> list of top symbol names with kinds
    pub subdirs: BTreeMap<String, OutlineNode>,
}

/// Add a file path into the outline tree, bounded by max_depth.
pub fn add_path_to_outline(
    root_node: &mut OutlineNode,
    file_path: &str,
    symbols_by_file: &HashMap<String, Vec<Symbol>>,
    max_depth: usize,
    norm_filter: &str,
) {
    let normalized = file_path.replace('\\', "/");
    let rel_path_str = if norm_filter.is_empty() {
        normalized.as_str()
    } else if normalized == norm_filter {
        Path::new(&normalized)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&normalized)
    } else if let Some(stripped) = normalized.strip_prefix(&format!("{norm_filter}/")) {
        stripped
    } else {
        return;
    };

    let path = Path::new(rel_path_str);
    let components: Vec<&str> = path
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .filter(|s| !s.is_empty())
        .collect();

    if components.is_empty() {
        return;
    }

    let mut curr = root_node;
    let depth = components.len();

    for (i, comp) in components.iter().enumerate() {
        if i == depth - 1 {
            // Leaf file: only insert if it is within max_depth
            if depth <= max_depth {
                let mut sym_tags = Vec::new();
                if let Some(syms) = symbols_by_file.get(&normalized) {
                    for s in syms.iter().take(5) {
                        sym_tags.push(format!("{} {}", s.kind, s.name));
                    }
                    if syms.len() > 5 {
                        sym_tags.push(format!("+{} more", syms.len() - 5));
                    }
                }
                curr.files.insert(comp.to_string(), sym_tags);
            }
        } else if i < max_depth {
            curr = curr.subdirs.entry(comp.to_string()).or_default();
        } else {
            break;
        }
    }
}

/// Format compact architectural outline of the repository.
pub fn format_codebase_outline(
    root_label: &str,
    files: &[FileFact],
    symbols_by_file: &HashMap<String, Vec<Symbol>>,
    max_depth: usize,
    path_filter: Option<&str>,
) -> String {
    let mut root_node = OutlineNode::default();
    let norm_filter = path_filter
        .map(|f| f.replace('\\', "/").trim_matches('/').to_string())
        .unwrap_or_default();

    for file in files {
        add_path_to_outline(
            &mut root_node,
            &file.path,
            symbols_by_file,
            max_depth,
            &norm_filter,
        );
    }

    let display_root = if norm_filter.is_empty() {
        format!("{root_label}/")
    } else {
        format!("{root_label}/{norm_filter}/")
    };

    let mut out = String::new();
    out.push_str(&format!("{display_root}\n"));
    render_outline_tree(&mut out, &root_node, "", 0, max_depth);
    out
}

pub fn render_outline_tree(
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
            out.push_str(&format!(
                "- `{}` ({}:{})\n",
                test.name, test.path, test.start_line
            ));
        }
        out.push('\n');
    }

    out
}

/// Format references list for callers/callees.
pub fn format_references(target_name: &str, refs: &[ReferenceSite], direction: &str) -> String {
    let mut out = String::new();
    let dir_label = if direction == "callers" {
        "Callers of"
    } else {
        "Callees called by"
    };
    out.push_str(&format!(
        "{dir_label} `{target_name}` ({} found):\n",
        refs.len()
    ));

    if refs.is_empty() {
        out.push_str("  (none)\n");
        return out;
    }

    for r in refs {
        let line_info = match r.start_line {
            Some(l) => format!(":{l}"),
            None => String::new(),
        };
        let other = if direction == "callers" {
            &r.from_symbol_name
        } else {
            &r.to_symbol_name
        };
        out.push_str(&format!(
            "- `{other}` [{}{line_info}] (kind: {})\n",
            r.path, r.kind
        ));
    }

    out
}

/// Formats FTS5 conceptual search results into token-dense markdown.
pub fn format_search_results(query: &str, results: &[SymbolSearchResult]) -> String {
    if results.is_empty() {
        return format!("No symbols found matching concept \"{query}\".");
    }

    let mut out = format!(
        "Found {} symbols matching concept \"{query}\":\n\n",
        results.len()
    );
    for r in results {
        let s = &r.symbol;
        let sig = s.signature.as_deref().unwrap_or(&s.name);
        out.push_str(&format!(
            "- {} `{}` [{}:{}-{}] (score: {:.2})\n",
            s.kind, s.name, s.path, s.start_line, s.end_line, r.score
        ));
        out.push_str(&format!("  Signature: {sig}\n"));
        if let Some(snippet) = &r.snippet {
            let clean_snip = snippet.replace('\r', "").trim().to_string();
            let first_line = clean_snip.lines().next().unwrap_or(&clean_snip);
            out.push_str(&format!("  Match: {first_line}\n"));
        } else if let Some(doc) = &s.doc_comment {
            let first_line = doc.lines().next().unwrap_or("").trim();
            if !first_line.is_empty() {
                out.push_str(&format!("  Doc: {first_line}\n"));
            }
        }
    }

    out
}

/// Format blast radius and likely test targets into token-dense markdown.
pub fn format_blast_radius(result: &BlastRadiusResult) -> String {
    if result.seed_type == "none"
        || (result.seeds.is_empty()
            && result.likely_tests.is_empty()
            && result.impacted_symbols.is_empty())
    {
        return "No uncommitted changes detected in git working tree. Pass a 'symbol' or 'file' parameter to analyze blast radius.".to_string();
    }

    let mut out = String::new();
    let seed_label = if result.seed_type == "file" {
        format!("Files: {}", result.seeds.join(", "))
    } else if result.seed_type == "symbol" {
        format!("Symbol: {}", result.seeds.join(", "))
    } else {
        format!("Seeds: {}", result.seeds.join(", "))
    };

    out.push_str(&format!("## Blast Radius & Test Impact ({seed_label})\n\n"));

    if !result.likely_tests.is_empty() {
        out.push_str(&format!(
            "### Likely Tests to Run ({} found)\n",
            result.likely_tests.len()
        ));
        for t in &result.likely_tests {
            out.push_str(&format!(
                "- `{}` [{}:{}] ({})\n",
                t.name, t.path, t.line, t.reason
            ));
        }
        out.push('\n');
    } else {
        out.push_str("### Likely Tests to Run\nNo direct or stem-matched tests found.\n\n");
    }

    if !result.impacted_symbols.is_empty() {
        out.push_str(&format!(
            "### Downstream Impact ({} symbols)\n",
            result.impacted_symbols.len()
        ));
        for s in &result.impacted_symbols {
            out.push_str(&format!(
                "- [depth {}] {} `{}` [{}:{}]\n",
                s.depth, s.kind, s.name, s.path, s.line
            ));
        }
    } else {
        out.push_str("### Downstream Impact\nNo downstream callers found within depth.\n");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ImpactedSymbol, TestTarget};

    #[test]
    fn test_format_file_skeleton() {
        let syms = vec![Symbol {
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
        }];

        let skeleton = format_file_skeleton("src/lib.rs", &syms, Some(35));
        assert!(skeleton.contains("/// Performs core work."));
        assert!(skeleton.contains("19 lines hidden: L11-L29"));
    }

    #[test]
    fn test_format_search_results() {
        let results = vec![SymbolSearchResult {
            symbol: Symbol {
                symbol_id: "s1".into(),
                file_id: "f1".into(),
                path: "src/parser.rs".into(),
                language: "rust".into(),
                name: "parse_tokens".into(),
                kind: "function".into(),
                signature: Some("pub fn parse_tokens()".into()),
                doc_comment: Some("Parses tokens from stream.".into()),
                visibility: Some("pub".into()),
                parent_symbol_id: None,
                start_line: 15,
                start_column: 0,
                end_line: 25,
                end_column: 1,
                start_byte: 100,
                end_byte: 250,
                body_start_line: None,
                body_start_column: None,
                body_end_line: None,
                body_end_column: None,
                body_start_byte: None,
                body_end_byte: None,
                body_hash: None,
                semantic_group: None,
                is_test: false,
                test_container: false,
            },
            score: -1.85,
            snippet: Some("Parses [tokens] from stream.".into()),
        }];

        let formatted = format_search_results("tokens", &results);
        assert!(formatted.contains("Found 1 symbols matching concept \"tokens\":"));
        assert!(
            formatted.contains("- function `parse_tokens` [src/parser.rs:15-25] (score: -1.85)")
        );
        assert!(formatted.contains("Match: Parses [tokens] from stream."));
    }

    #[test]
    fn test_format_blast_radius() {
        let res = BlastRadiusResult {
            seed_type: "symbol".into(),
            seeds: vec!["do_work".into()],
            likely_tests: vec![TestTarget {
                name: "test_do_work".into(),
                path: "tests/work_test.rs".into(),
                line: 15,
                reason: "transitive caller [depth 1]".into(),
            }],
            impacted_symbols: vec![ImpactedSymbol {
                name: "caller_fn".into(),
                kind: "function".into(),
                path: "src/caller.rs".into(),
                line: 42,
                depth: 1,
            }],
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("## Blast Radius & Test Impact (Symbol: do_work)"));
        assert!(formatted.contains("### Likely Tests to Run (1 found)"));
        assert!(
            formatted
                .contains("- `test_do_work` [tests/work_test.rs:15] (transitive caller [depth 1])")
        );
        assert!(formatted.contains("- [depth 1] function `caller_fn` [src/caller.rs:42]"));
    }
}
