use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::models::{
    BlastRadiusResult, ContextSlice, ImpactedSymbol, ReferenceSite, Symbol, SymbolSearchResult,
    TestTarget,
};

/// Format progressive disclosure file skeleton with implementation bodies stripped.
pub fn format_file_skeleton(
    file_path: &str,
    symbols: &[Symbol],
    line_count: Option<usize>,
    parse_errors: usize,
) -> String {
    let mut out = String::new();
    let lines_str = match line_count {
        Some(c) => format!(" (Lines 1-{c})"),
        None => String::new(),
    };
    out.push_str(&format!("// File: {file_path}{lines_str}\n\n"));

    if parse_errors > 0 {
        let noun = if parse_errors == 1 { "error" } else { "errors" };
        out.push_str(&format!(
            "// {parse_errors} parse {noun}: symbols may be incomplete\n\n"
        ));
    }

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

fn sanitize_skeleton_sig<'a>(sig: &'a str, name: &'a str) -> &'a str {
    let clean = if let Some(idx) = sig.find('{') {
        sig[..idx].trim_end()
    } else {
        sig.trim_end()
    };
    let trimmed = clean.trim_end_matches(';').trim_end();
    if trimmed.is_empty() { name } else { trimmed }
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

    // Doc comment: cap at 3 lines to prevent dumping huge blocks
    if let Some(ref doc) = sym.doc_comment {
        let lines: Vec<_> = doc.lines().collect();
        let cap = 3;
        for line in lines.iter().take(cap) {
            out.push_str(&format!("{indent}/// {line}\n"));
        }
        if lines.len() > cap {
            out.push_str(&format!(
                "{indent}/// ... ({} more lines)\n",
                lines.len() - cap
            ));
        }
    }

    let span_str = format!("L{}-{}", sym.start_line, sym.end_line);

    // Check if this symbol is a container (class, struct, trait, enum, etc.)
    let children = children_map.get(&Some(sym.symbol_id.clone()));

    if is_container_kind(&sym.kind) && children.is_some() {
        let raw_sig = sym.signature.as_deref().unwrap_or(&sym.name);
        let sig = sanitize_skeleton_sig(raw_sig, &sym.name);
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
            let raw_sig = sym.signature.as_deref().unwrap_or(&sym.name);
            let sig = sanitize_skeleton_sig(raw_sig, &sym.name);
            let b_start = sym.body_start_line.unwrap_or(sym.start_line);
            let b_end = sym.body_end_line.unwrap_or(sym.end_line);

            if count > 1 {
                out.push_str(&format!(
                    "{indent}{sig} {{ /* {count} lines hidden: L{b_start}-L{b_end} */ }}\n"
                ));
            } else {
                out.push_str(&format!("{indent}{sig}; // {span_str}\n"));
            }
        } else if let Some(ref raw_sig) = sym.signature {
            let sig = sanitize_skeleton_sig(raw_sig, &sym.name);
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
    } else if normalized.eq_ignore_ascii_case(norm_filter) {
        Path::new(&normalized)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&normalized)
    } else if normalized.len() > norm_filter.len()
        && normalized.as_bytes()[norm_filter.len()] == b'/'
        && normalized[..norm_filter.len()].eq_ignore_ascii_case(norm_filter)
    {
        &normalized[norm_filter.len() + 1..]
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
                if !sym_tags.is_empty() {
                    curr.files.insert(comp.to_string(), sym_tags);
                }
            }
        } else if i < max_depth {
            curr = curr.subdirs.entry(comp.to_string()).or_default();
        } else {
            break;
        }
    }
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

/// Format symbol body with metadata header, signature, and body content.
pub fn format_symbol_body(symbol: &Symbol, body: &str) -> String {
    let body_hash = crate::edit::hash_content(body);
    let mut out = format!(
        "// {}:{}-{} ({}) body_hash={body_hash}\n",
        symbol.path, symbol.start_line, symbol.end_line, symbol.name
    );
    if let Some(ref sig) = symbol.signature {
        out.push_str(sig);
        if !sig.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Format surgical context bundle for a symbol.
pub fn format_context_slice(slice: &ContextSlice) -> String {
    let sym = &slice.target_symbol;
    let mut out = String::new();
    let body_hash = crate::edit::hash_content(&slice.target_body);

    out.push_str(&format!(
        "### Target: `{}` ({}:{}-{}) body_hash={body_hash}\n\n",
        sym.name, sym.path, sym.start_line, sym.end_line
    ));

    if let Some(ref sig) = sym.signature {
        out.push_str(&format!("Signature: `{sig}`\n\n"));
    }

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
        if slice.callee_signatures.len() >= 10 {
            out.push_str("[Showing 10 dependencies (limit reached)]\n");
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
        if slice.related_tests.len() >= 5 {
            out.push_str("[Showing 5 tests (limit reached)]\n");
        }
        out.push('\n');
    }

    out
}

fn cap_notice(shown: usize, limit: usize) -> String {
    let advice = if limit >= crate::queries::MAX_RESULT_LIMIT {
        "narrow the query to see more"
    } else {
        "increase limit to see more"
    };
    format!("\n[Showing {shown} results (limit reached); {advice}.]\n")
}

/// Format references list for callers/callees with optional limit footer.
pub fn format_references(
    target_name: &str,
    refs: &[ReferenceSite],
    direction: &str,
    limit: usize,
) -> String {
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

    if refs.len() >= limit {
        out.push_str(&cap_notice(refs.len(), limit));
    }

    out
}

/// Formats exact or FTS fallback symbol results with transparent header labeling.
pub fn format_find_symbol_results(
    query: &str,
    exact_matches: &[Symbol],
    fts_matches: &[SymbolSearchResult],
    limit: usize,
) -> String {
    if !exact_matches.is_empty() {
        let mut out = format!(
            "Found {} symbols matching \"{query}\":\n\n",
            exact_matches.len()
        );
        for s in exact_matches {
            let sig = s.signature.as_deref().unwrap_or(&s.name);
            out.push_str(&format!(
                "- {} `{}` [{}:{}-{}]\n",
                s.kind, s.name, s.path, s.start_line, s.end_line
            ));
            out.push_str(&format!("  Signature: {sig}\n"));
            if let Some(doc) = &s.doc_comment {
                let first = doc.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    out.push_str(&format!("  Doc: {first}\n"));
                }
            }
        }
        if exact_matches.len() >= limit {
            out.push_str(&cap_notice(exact_matches.len(), limit));
        }
        out
    } else if !fts_matches.is_empty() {
        let mut out = format!(
            "No exact name match; {} full-text matches for \"{query}\":\n\n",
            fts_matches.len()
        );
        for r in fts_matches {
            let s = &r.symbol;
            let sig = s.signature.as_deref().unwrap_or(&s.name);
            out.push_str(&format!(
                "- {} `{}` [{}:{}-{}] (score: {:.2})\n",
                s.kind, s.name, s.path, s.start_line, s.end_line, r.score
            ));
            out.push_str(&format!("  Signature: {sig}\n"));
            if let Some(snippet) = &r.snippet {
                let clean = snippet.replace('\r', "").trim().to_string();
                let first = clean.lines().next().unwrap_or(&clean);
                out.push_str(&format!("  Match: {first}\n"));
            } else if let Some(doc) = &s.doc_comment {
                let first = doc.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    out.push_str(&format!("  Doc: {first}\n"));
                }
            }
        }
        if fts_matches.len() >= limit {
            out.push_str(&cap_notice(fts_matches.len(), limit));
        }
        out
    } else {
        format!("No symbols found matching \"{query}\".\n")
    }
}

/// Format available structural fact & literal categories.
pub fn format_fact_categories(categories: &[(String, usize)]) -> String {
    if categories.is_empty() {
        return "No structural facts or literals indexed in this repository.".to_string();
    }
    let mut out = format!(
        "Available structural fact & literal categories ({} found):\n\n",
        categories.len()
    );
    for (name, count) in categories {
        out.push_str(&format!("- `{name}` ({count} occurrences)\n"));
    }
    out
}

/// Format structural facts and matching literals into token-dense markdown.
pub fn format_structural_facts(
    facts: &[crate::models::StructuralFact],
    literals: &[crate::models::LiteralFact],
    category: &str,
) -> String {
    let mut out = format!(
        "Structural facts for '{category}' ({} found):\n",
        facts.len()
    );
    for f in facts {
        let label = f.key.as_deref().unwrap_or(&f.capture_name);
        let parent = f
            .containing_symbol_name
            .as_deref()
            .map(|p| format!(", in: {p}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- {label} [{}:{}] (pattern: {}{parent})\n",
            f.path, f.start_line, f.pattern_id
        ));
    }
    if !literals.is_empty() {
        out.push_str(&format!(
            "\nMatching literals ({} found):\n",
            literals.len()
        ));
        for l in literals {
            out.push_str(&format!(
                "- \"{}\" [{}:{}] (kind: {})\n",
                l.literal_text, l.path, l.start_line, l.kind
            ));
        }
    }
    out
}

/// Format result of atomic symbol body replacement.
pub fn format_replace_symbol_result(res: &crate::edit::EditResult) -> String {
    let syntax_line = if res.syntax_checked {
        "Syntax: Verified"
    } else {
        "Syntax: Skipped (grammar not available for file extension)"
    };
    format!(
        "Successfully replaced body of `{}` in `{}`.\nOld Hash: {}\nNew Hash: {}\nBytes Written: {}\n{}",
        res.symbol_name,
        res.file_path,
        res.old_body_hash,
        res.new_body_hash,
        res.bytes_written,
        syntax_line
    )
}

/// Formats FTS5 conceptual search results into token-dense markdown.
pub fn format_search_results(query: &str, results: &[SymbolSearchResult], limit: usize) -> String {
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

    if results.len() >= limit {
        out.push_str(&cap_notice(results.len(), limit));
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

    const MAX_COMPACT_TESTS: usize = 20;
    const MAX_COMPACT_IMPACTED: usize = 50;

    if !result.likely_tests.is_empty() {
        let total = result.likely_tests.len();
        if total > MAX_COMPACT_TESTS {
            out.push_str(&format!(
                "### Likely Tests to Run ({} found - showing top {})\n",
                total, MAX_COMPACT_TESTS
            ));
        } else {
            out.push_str(&format!("### Likely Tests to Run ({} found)\n", total));
        }

        let mut tests_by_file: std::collections::BTreeMap<&str, Vec<&TestTarget>> =
            std::collections::BTreeMap::new();
        let mut file_order = Vec::new();
        for t in result.likely_tests.iter().take(MAX_COMPACT_TESTS) {
            if !tests_by_file.contains_key(t.path.as_str()) {
                file_order.push(t.path.as_str());
            }
            tests_by_file.entry(t.path.as_str()).or_default().push(t);
        }

        for path in file_order {
            out.push_str(&format!("{path}:\n"));
            if let Some(tests) = tests_by_file.get(path) {
                for t in tests {
                    out.push_str(&format!(
                        "  - `{}` [line {}] ({})\n",
                        t.name, t.line, t.reason
                    ));
                }
            }
        }

        if total > MAX_COMPACT_TESTS {
            out.push_str(&format!(
                "... {} more likely tests; narrow the target. CLI --json shows the full returned list.\n",
                total - MAX_COMPACT_TESTS
            ));
        }
        out.push('\n');
    } else {
        out.push_str("### Likely Tests to Run\nNo direct or stem-matched tests found.\n\n");
    }

    if !result.impacted_symbols.is_empty() {
        let total = result.impacted_symbols.len();
        let mut visible = Vec::new();
        let mut low_signal_count = 0;
        for s in &result.impacted_symbols {
            if matches!(s.kind.as_str(), "import" | "module" | "namespace") {
                low_signal_count += 1;
            } else {
                visible.push(s);
            }
        }

        if result.traversal_ceiling_reached || total >= 200 {
            out.push_str("### Downstream Impact (200+ symbols - traversal ceiling reached; increase depth/limit or narrow target)\n");
        } else {
            out.push_str(&format!("### Downstream Impact ({} symbols)\n", total));
        }

        if visible.is_empty() && low_signal_count > 0 {
            let row_word = if low_signal_count == 1 {
                "row (import/module)"
            } else {
                "rows (imports/modules)"
            };
            out.push_str(&format!(
                "All impacted symbols are imports/modules; {low_signal_count} low-signal {row_word} hidden; available in CLI --json.\n"
            ));
        } else {
            let visible_total = visible.len();
            let showing_count = visible_total.min(MAX_COMPACT_IMPACTED);

            let mut syms_by_file: std::collections::BTreeMap<&str, Vec<&ImpactedSymbol>> =
                std::collections::BTreeMap::new();
            let mut file_order = Vec::new();
            for s in visible.iter().take(showing_count) {
                if !syms_by_file.contains_key(s.path.as_str()) {
                    file_order.push(s.path.as_str());
                }
                syms_by_file.entry(s.path.as_str()).or_default().push(s);
            }

            for path in file_order {
                out.push_str(&format!("{path}:\n"));
                if let Some(syms) = syms_by_file.get(path) {
                    for s in syms {
                        out.push_str(&format!(
                            "  - [depth {}] {} `{}` [line {}]\n",
                            s.depth, s.kind, s.name, s.line
                        ));
                    }
                }
            }

            if visible_total > MAX_COMPACT_IMPACTED {
                out.push_str(&format!(
                    "... {} more impacted symbols; narrow the target. CLI --json shows the full returned list.\n",
                    visible_total - MAX_COMPACT_IMPACTED
                ));
            }
            if low_signal_count > 0 {
                let row_word = if low_signal_count == 1 {
                    "row (import/module)"
                } else {
                    "rows (imports/modules)"
                };
                out.push_str(&format!(
                    "... {low_signal_count} low-signal {row_word} hidden; available in CLI --json.\n"
                ));
            }
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

    fn structural_fact(key: Option<&str>) -> crate::models::StructuralFact {
        crate::models::StructuralFact {
            structural_fact_id: "sf1".into(),
            path: ".codex/config.toml".into(),
            language: "toml".into(),
            pattern_id: "toml.key_value.v1".into(),
            capture_name: "key_value".into(),
            node_kind: "table".into(),
            key: key.map(str::to_string),
            containing_symbol_name: None,
            start_line: 2,
            end_line: 2,
            confidence: 1.0,
        }
    }

    #[test]
    fn format_structural_facts_prints_key_and_falls_back_to_capture_name() {
        let with_key = format_structural_facts(
            &[structural_fact(Some("mcp_servers.code-kb.command"))],
            &[],
            "config",
        );
        assert!(with_key.contains(
            "- mcp_servers.code-kb.command [.codex/config.toml:2] (pattern: toml.key_value.v1)"
        ));

        let without_key = format_structural_facts(&[structural_fact(None)], &[], "config");
        assert!(
            without_key.contains("- key_value [.codex/config.toml:2] (pattern: toml.key_value.v1)")
        );
    }

    #[test]
    fn file_skeleton_reports_parse_errors() {
        let two = format_file_skeleton("src/lib.rs", &[], Some(35), 2);
        assert!(two.contains("// 2 parse errors: symbols may be incomplete"));

        let one = format_file_skeleton("src/lib.rs", &[], Some(35), 1);
        assert!(one.contains("// 1 parse error: symbols may be incomplete"));

        let none = format_file_skeleton("src/lib.rs", &[], Some(35), 0);
        assert!(!none.contains("parse error"));
    }

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

        let skeleton = format_file_skeleton("src/lib.rs", &syms, Some(35), 0);
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

        let formatted = format_search_results("tokens", &results, 20);
        assert!(formatted.contains("Found 1 symbols matching concept \"tokens\":"));
        assert!(
            formatted.contains("- function `parse_tokens` [src/parser.rs:15-25] (score: -1.85)")
        );
        assert!(formatted.contains("Match: Parses [tokens] from stream."));
    }

    #[test]
    fn test_format_search_results_discloses_a_reached_limit() {
        let results = vec![SymbolSearchResult {
            symbol: sample_symbol("parse_tokens"),
            score: 0.0,
            snippet: None,
        }];

        let formatted = format_search_results("tokens", &results, 1);
        assert!(
            formatted.contains("[Showing 1 results (limit reached); increase limit to see more.]")
        );

        let at_ceiling =
            format_search_results("tokens", &results, crate::queries::MAX_RESULT_LIMIT);
        assert!(!at_ceiling.contains("limit reached"));

        let full: Vec<SymbolSearchResult> = (0..crate::queries::MAX_RESULT_LIMIT)
            .map(|_| SymbolSearchResult {
                symbol: sample_symbol("parse_tokens"),
                score: 0.0,
                snippet: None,
            })
            .collect();
        let capped = format_search_results("tokens", &full, crate::queries::MAX_RESULT_LIMIT);
        assert!(capped.contains("(limit reached); narrow the query to see more.]"));
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
            traversal_ceiling_reached: false,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("## Blast Radius & Test Impact (Symbol: do_work)"));
        assert!(formatted.contains("### Likely Tests to Run (1 found)"));
        assert!(formatted.contains(
            "tests/work_test.rs:\n  - `test_do_work` [line 15] (transitive caller [depth 1])"
        ));
        assert!(formatted.contains("src/caller.rs:\n  - [depth 1] function `caller_fn` [line 42]"));
    }

    #[test]
    fn test_format_blast_radius_grouped_and_capped() {
        let mut likely_tests = Vec::new();
        for i in 1..=25 {
            likely_tests.push(TestTarget {
                name: format!("test_{i}"),
                path: format!("tests/test_{}.rs", (i % 3) + 1),
                line: i * 10,
                reason: "direct caller".into(),
            });
        }

        let impacted_symbols = vec![
            ImpactedSymbol {
                name: "use_foo".into(),
                kind: "import".into(),
                path: "src/service.rs".into(),
                line: 1,
                depth: 1,
            },
            ImpactedSymbol {
                name: "service_fn".into(),
                kind: "function".into(),
                path: "src/service.rs".into(),
                line: 20,
                depth: 1,
            },
            ImpactedSymbol {
                name: "api_handler".into(),
                kind: "function".into(),
                path: "src/api.rs".into(),
                line: 45,
                depth: 2,
            },
        ];

        let res = BlastRadiusResult {
            seed_type: "file".into(),
            seeds: vec!["src/lib.rs".into()],
            likely_tests,
            impacted_symbols,
            traversal_ceiling_reached: false,
        };

        let formatted = format_blast_radius(&res);

        assert!(formatted.contains("### Likely Tests to Run (25 found - showing top 20)"));
        assert!(formatted.contains(
            "... 5 more likely tests; narrow the target. CLI --json shows the full returned list."
        ));

        assert!(formatted.contains("tests/test_1.rs:\n"));
        assert!(formatted.contains("  - `test_"));

        assert!(!formatted.contains("use_foo"));
        assert!(
            formatted
                .contains("... 1 low-signal row (import/module) hidden; available in CLI --json.")
        );
        assert!(formatted.contains("src/service.rs:\n"));
        assert!(formatted.contains("  - [depth 1] function `service_fn` [line 20]"));
    }

    #[test]
    fn test_format_replace_symbol_result_shows_syntax_status() {
        let res_checked = crate::edit::EditResult {
            symbol_name: "my_fn".into(),
            file_path: "src/lib.rs".into(),
            old_body_hash: "aaa".into(),
            new_body_hash: "bbb".into(),
            bytes_written: 120,
            syntax_checked: true,
        };
        let out_checked = format_replace_symbol_result(&res_checked);
        assert!(out_checked.contains("Syntax: Verified"));

        let res_skipped = crate::edit::EditResult {
            symbol_name: "my_fn".into(),
            file_path: "src/script.rb".into(),
            old_body_hash: "aaa".into(),
            new_body_hash: "bbb".into(),
            bytes_written: 120,
            syntax_checked: false,
        };
        let out_skipped = format_replace_symbol_result(&res_skipped);
        assert!(out_skipped.contains("Syntax: Skipped (grammar not available for file extension)"));
    }

    fn sample_symbol(name: &str) -> Symbol {
        Symbol {
            symbol_id: format!("id_{name}"),
            file_id: "f1".into(),
            path: "src/lib.rs".into(),
            language: "rust".into(),
            name: name.into(),
            kind: "function".into(),
            signature: Some(format!("pub fn {name}()")),
            doc_comment: None,
            visibility: Some("pub".into()),
            parent_symbol_id: None,
            start_line: 1,
            start_column: 0,
            end_line: 10,
            end_column: 1,
            start_byte: 0,
            end_byte: 100,
            body_start_line: Some(2),
            body_start_column: Some(0),
            body_end_line: Some(9),
            body_end_column: Some(1),
            body_start_byte: Some(10),
            body_end_byte: Some(99),
            body_hash: None,
            semantic_group: None,
            is_test: false,
            test_container: false,
        }
    }

    fn sample_context_slice() -> ContextSlice {
        ContextSlice {
            target_symbol: sample_symbol("target_fn"),
            target_body: "    println!(\"hello\");\n".into(),
            callee_signatures: Vec::new(),
            related_types: Vec::new(),
            related_tests: Vec::new(),
        }
    }

    #[test]
    fn test_context_slice_shows_truncation_notice_when_caps_hit() {
        let mut slice = sample_context_slice();
        slice.callee_signatures = (1..=10).map(|i| format!("fn callee_{i}()")).collect();
        let text = format_context_slice(&slice);
        assert!(text.contains("[Showing 10 dependencies (limit reached)]"));

        let mut slice_tests = sample_context_slice();
        slice_tests.related_tests = (1..=5)
            .map(|i| {
                let mut sym = sample_symbol(&format!("test_fn_{i}"));
                sym.path = format!("tests/test_{i}.rs");
                sym.is_test = true;
                sym
            })
            .collect();
        let text_tests = format_context_slice(&slice_tests);
        assert!(text_tests.contains("[Showing 5 tests (limit reached)]"));
    }

    #[test]
    fn test_blast_radius_shows_traversal_ceiling_at_200_symbols() {
        let impacted_symbols = (1..=200)
            .map(|i| ImpactedSymbol {
                name: format!("sym_{i}"),
                kind: "function".into(),
                path: format!("src/mod_{}.rs", i % 10),
                line: i,
                depth: 1,
            })
            .collect();

        let res = BlastRadiusResult {
            seed_type: "symbol".into(),
            seeds: vec!["root_fn".into()],
            likely_tests: Vec::new(),
            impacted_symbols,
            traversal_ceiling_reached: true,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains(
            "### Downstream Impact (200+ symbols - traversal ceiling reached; increase depth/limit or narrow target)\n"
        ));
    }
}
