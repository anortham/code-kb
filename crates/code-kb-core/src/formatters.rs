use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::models::{
    BlastRadiusResult, ContextSlice, ImpactedSymbol, ReferenceSite, SearchExplain, Symbol,
    SymbolSearchResult, TestTarget,
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
            render_symbol_skeleton(&mut out, root, None, &children_map, 0);
        }
    } else {
        // If parent relationships are missing or flat, render all sorted by line
        for s in symbols {
            render_symbol_skeleton(&mut out, s, None, &children_map, 0);
        }
    }

    out
}

fn is_container_kind(kind: &str) -> bool {
    let hides_its_body = matches!(
        kind,
        "function" | "method" | "constructor" | "destructor" | "operator"
    );
    !is_skippable_kind(kind) && !hides_its_body
}

/// The kind word to show for a symbol. julie reuses code kinds for markup and data files: an
/// HTML element and a SQL table are `class` rows, a Markdown heading a `module`, a Markdown
/// link an `import`. `queries::KIND_FILTER` mirrors it in SQL.
pub(crate) fn display_kind(sym: &Symbol) -> &str {
    match (sym.language.as_str(), sym.kind.as_str()) {
        ("html", "class") => "element",
        ("sql", "class") => "table",
        ("markdown", "module") => "section",
        ("markdown", "import") => "link",
        (language, "property")
            if language != "fsharp"
                && sym.signature.as_deref().is_some_and(|sig| {
                    ["self.", "this.", "cls."]
                        .iter()
                        .any(|p| sig.starts_with(p))
                }) =>
        {
            "attribute"
        }
        (_, kind) => kind,
    }
}

fn is_skippable_kind(kind: &str) -> bool {
    matches!(kind, "variable" | "parameter" | "import")
}

/// A value row on one line: the declaration before its `=` stays whole, because its type is part
/// of the interface, and the value is cut to fit about 120 characters.
fn value_row_signature(sig: &str) -> std::borrow::Cow<'_, str> {
    let sig = sig.trim_end().trim_end_matches(';').trim_end();
    if !sig.contains('\n') && sig.chars().count() <= 120 {
        return sig.into();
    }
    let joined = one_line(sig);
    let Some(equals) = value_equals(&joined) else {
        if joined.chars().count() <= 120 {
            return joined.into();
        }
        let cut: String = joined.chars().take(119).collect();
        return format!("{}…", cut.trim_end()).into();
    };
    let (declaration, value) = joined.split_at(equals);
    let budget = 120usize.saturating_sub(declaration.chars().count()).max(40);
    if value.chars().count() <= budget {
        return joined.into();
    }
    let cut: String = value.chars().take(budget - 1).collect();
    format!("{declaration}{}…", cut.trim_end()).into()
}

/// Joins a multi-line declaration into one line, without trailing `  # ` or `  // ` comments.
fn one_line(sig: &str) -> String {
    let mut joined = sig
        .lines()
        .map(|line| {
            let code = ["  # ", "  // "]
                .iter()
                .filter_map(|marker| line.find(marker))
                .min()
                .map_or(line, |at| &line[..at]);
            code.trim().trim_end_matches('\\').trim_end()
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    for (from, to) in [
        ("( ", "("),
        ("[ ", "["),
        ("{ ", "{"),
        (", )", ")"),
        (", ]", "]"),
        (", }", "}"),
        (" )", ")"),
        (" ]", "]"),
        (" }", "}"),
    ] {
        joined = joined.replace(from, to);
    }
    joined
}

/// Whether `sig` assigns a value: an `=` outside brackets, before any `{`, that is not part of
/// `==`, `=>`, `>=`, `^=`, or another operator.
fn assigns_a_value(sig: &str) -> bool {
    value_equals(sig).is_some()
}

/// The byte index of the `=` that assigns a value, as [`assigns_a_value`] defines it.
fn value_equals(sig: &str) -> Option<usize> {
    let bytes = sig.as_bytes();
    let mut depth = 0usize;
    for (i, &byte) in bytes.iter().enumerate() {
        match byte {
            b'(' | b'[' => depth += 1,
            b')' | b']' => depth = depth.saturating_sub(1),
            b'{' if depth == 0 => return None,
            b'=' if depth == 0 => {
                let before = i.checked_sub(1).map(|j| bytes[j]);
                let after = bytes.get(i + 1).copied();
                let operator_before = before.is_some_and(|b| b"<>!=^*~|$+-/%&:?".contains(&b));
                let operator_after = after.is_some_and(|b| b == b'=' || b == b'>');
                if !operator_before && !operator_after {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn sanitize_skeleton_sig<'a>(sig: &'a str, sym: &'a Symbol) -> std::borrow::Cow<'a, str> {
    if matches!(
        sym.kind.as_str(),
        "variable" | "constant" | "property" | "field" | "enum_member"
    ) && assigns_a_value(sig)
    {
        return value_row_signature(sig);
    }
    container_signature(sig, sym)
}

/// The signature cut at its first `{`, which opens the body that the skeleton renders.
fn container_signature<'a>(sig: &'a str, sym: &'a Symbol) -> std::borrow::Cow<'a, str> {
    let expression_arrow = if sym.language == "csharp"
        && matches!(
            sym.kind.as_str(),
            "method" | "function" | "constructor" | "operator"
        ) {
        sig.match_indices("=>")
            .find_map(|(idx, _)| sig[..idx].trim_end().ends_with(')').then_some(idx))
    } else {
        None
    };
    let cutoff = sig
        .find('{')
        .into_iter()
        .chain(expression_arrow)
        .min()
        .unwrap_or(sig.len());
    let clean = sig[..cutoff].trim_end();
    let trimmed = clean.trim_end_matches(';').trim_end();
    if trimmed.is_empty() {
        sym.name.as_str().into()
    } else {
        trimmed.into()
    }
}

fn signature_without_duplicate_body<'a>(signature: &'a str, body: &str) -> &'a str {
    let body = body.trim();
    if body.is_empty() {
        return signature;
    }

    let trimmed_signature = signature.trim_end();
    trimmed_signature
        .strip_suffix(body)
        .map(str::trim_end)
        .unwrap_or(signature)
}

/// The kind word for a leaf row whose signature does not spell it. A C++ Qt signal is declared
/// under a `Q_SIGNALS:` label, so its signature reads like a method; the `event` kind goes into
/// the trailing comment instead.
fn unspelled_kind(sym: &Symbol) -> &'static str {
    if sym.kind != "event" {
        return "";
    }
    let spelled = sym.signature.as_deref().is_some_and(|sig| {
        sig.split_whitespace()
            .any(|word| word == "signal" || word == "event")
    });
    if spelled { "" } else { "event " }
}

/// `in run(), ` for an attribute that a method assigns, so the skeleton does not present it as
/// a declaration of the class body.
fn assigning_method(
    sym: &Symbol,
    parent: Option<&Symbol>,
    children_map: &HashMap<Option<String>, Vec<&Symbol>>,
) -> String {
    if !sym
        .signature
        .as_deref()
        .is_some_and(|sig| sig.starts_with("self."))
    {
        return String::new();
    }
    parent
        .and_then(|parent| children_map.get(&Some(parent.symbol_id.clone())))
        .and_then(|siblings| {
            siblings.iter().find(|method| {
                matches!(method.kind.as_str(), "method" | "function" | "constructor")
                    && method.start_line <= sym.start_line
                    && sym.end_line <= method.end_line
            })
        })
        .map_or_else(String::new, |method| format!("in {}(), ", method.name))
}

/// julie gives other languages' block locals, markup elements, and data keys a `variable` row
/// under a parent too; only a Python module or class body declares names that way.
fn is_shown(sym: &Symbol, parent: Option<&Symbol>) -> bool {
    let python_declaration = sym.kind == "variable"
        && sym.language == "python"
        && parent.is_none_or(|parent| parent.kind == "class");
    !is_skippable_kind(&sym.kind) || python_declaration
}

/// `; defines `a`, `b`` for the functions and classes declared inside a hidden body, so a view
/// or wrapper defined in a factory stays visible. Lambdas are left out.
/// C and C++ cannot define a function inside a function, so such a row comes from a macro the
/// parser misread (`JSON_CATCH (...) { }`). Test sections such as doctest `SECTION` are real.
fn is_misparsed_c_function(child: &Symbol) -> bool {
    matches!(child.language.as_str(), "c" | "cpp")
        && matches!(child.kind.as_str(), "function" | "method")
        && !child.is_test
        && !child.test_container
}

fn nested_definitions(children: &[&Symbol]) -> String {
    let names: Vec<String> = children
        .iter()
        .filter(|child| {
            matches!(
                child.kind.as_str(),
                "function" | "method" | "class" | "struct"
            )
        })
        .filter(|child| {
            !child
                .signature
                .as_deref()
                .is_some_and(|sig| sig.starts_with("lambda"))
        })
        .filter(|child| !is_misparsed_c_function(child))
        .map(|child| format!("`{}`", child.name))
        .fold(Vec::new(), |mut names, name| {
            if !names.contains(&name) {
                names.push(name);
            }
            names
        });
    match names.len() {
        0 => String::new(),
        1..=5 => format!("; defines {}", names.join(", ")),
        n => format!("; defines {}, +{} more", names[..5].join(", "), n - 5),
    }
}

fn render_symbol_skeleton(
    out: &mut String,
    sym: &Symbol,
    parent: Option<&Symbol>,
    children_map: &HashMap<Option<String>, Vec<&Symbol>>,
    indent_level: usize,
) {
    if !is_shown(sym, parent) {
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
    let leaf_note = format!(
        "{}{}{span_str}",
        unspelled_kind(sym),
        assigning_method(sym, parent, children_map)
    );

    let children = children_map.get(&Some(sym.symbol_id.clone()));

    let spans_multiple_lines = sym.end_line > sym.start_line;

    if is_container_kind(&sym.kind)
        && children.is_some_and(|list| list.iter().any(|child| is_shown(child, Some(sym))))
        && (spans_multiple_lines || sym.kind == "enum")
    {
        let raw_sig = sym.signature.as_deref().unwrap_or(&sym.name);
        let sig = container_signature(raw_sig, sym);
        out.push_str(&format!("{indent}{sig} {{\n"));
        if let Some(child_list) = children {
            for child in child_list {
                render_symbol_skeleton(out, child, Some(sym), children_map, indent_level + 1);
            }
        }
        out.push_str(&format!("{indent}}} // {span_str}\n\n"));
    } else {
        if let Some(count) = sym.hidden_body_line_count() {
            let raw_sig = sym.signature.as_deref().unwrap_or(&sym.name);
            let sig = if count > 1 {
                container_signature(raw_sig, sym)
            } else {
                sanitize_skeleton_sig(raw_sig, sym)
            };
            let b_start = sym.body_start_line.unwrap_or(sym.start_line);
            let b_end = sym.body_end_line.unwrap_or(sym.end_line);

            if count > 1 {
                let defines = nested_definitions(children.map(Vec::as_slice).unwrap_or_default());
                out.push_str(&format!(
                    "{indent}{sig} {{ /* {count} lines hidden: L{b_start}-L{b_end}{defines} */ }}\n"
                ));
            } else {
                out.push_str(&format!("{indent}{sig}; // {leaf_note}\n"));
            }
        } else if let Some(ref raw_sig) = sym.signature {
            let sig = sanitize_skeleton_sig(raw_sig, sym);
            out.push_str(&format!("{indent}{sig}; // {leaf_note}\n"));
        } else {
            out.push_str(&format!(
                "{indent}{} {sym_name}; // {span_str}\n",
                display_kind(sym),
                sym_name = sym.name
            ));
        }
        if sym.kind == "namespace"
            && let Some(child_list) = children
        {
            for child in child_list {
                render_symbol_skeleton(out, child, Some(sym), children_map, indent_level);
            }
        }
    }
}

/// Node representing directory or file in codebase outline tree.
#[derive(Default)]
pub struct OutlineNode {
    pub files: BTreeMap<String, Vec<String>>, // file_name -> list of top symbol names with kinds
    pub subdirs: BTreeMap<String, OutlineNode>,
    /// Files under this directory that lie below the depth limit.
    pub hidden_files: usize,
    /// Indexed files in this directory with no function, class, or test to list.
    pub plain_files: Vec<String>,
    /// Files under this directory that no extractor reads.
    pub unsupported_files: usize,
}

/// The path components of `file_path` below the outline's path filter, or `None` for a path
/// outside it.
fn outline_components(file_path: &str, norm_filter: &str) -> Option<Vec<String>> {
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
        return None;
    };

    let path = Path::new(rel_path_str);
    let components: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_str().unwrap_or(""))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();

    if components.is_empty() {
        return None;
    }

    Some(components)
}

/// Counts a file no extractor reads on the deepest folder the outline already shows for it, or
/// on its top-level folder. Call it after every indexed file is added.
pub fn add_unsupported_to_outline(
    root_node: &mut OutlineNode,
    file_path: &str,
    max_depth: usize,
    norm_filter: &str,
) {
    let Some(components) = outline_components(file_path, norm_filter) else {
        return;
    };
    let mut curr = root_node;
    for (i, comp) in components
        .iter()
        .take(components.len() - 1)
        .take(max_depth)
        .enumerate()
    {
        if i > 0 && !curr.subdirs.contains_key(comp) {
            break;
        }
        curr = curr.subdirs.entry(comp.clone()).or_default();
    }
    curr.unsupported_files += 1;
}

/// Add a file path into the outline tree, bounded by max_depth.
pub fn add_path_to_outline(
    root_node: &mut OutlineNode,
    file_path: &str,
    symbols_by_file: &HashMap<String, Vec<Symbol>>,
    counts: &HashMap<String, crate::queries::OutlineCounts>,
    max_depth: usize,
    norm_filter: &str,
) {
    let Some(components) = outline_components(file_path, norm_filter) else {
        return;
    };
    let normalized = file_path.replace('\\', "/");

    let mut curr = root_node;
    let depth = components.len();

    for (i, comp) in components.iter().enumerate() {
        if i == depth - 1 {
            if depth > max_depth {
                curr.hidden_files += 1;
            } else {
                let mut sym_tags: Vec<String> = symbols_by_file
                    .get(&normalized)
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_test && !s.test_container)
                    .take(5)
                    .map(|s| format!("{} {}", display_kind(s), s.name))
                    .collect();
                let count = counts.get(&normalized).copied().unwrap_or_default();
                if count.definitions > sym_tags.len() {
                    sym_tags.push(format!("+{} more", count.definitions - sym_tags.len()));
                }
                if count.tests > 0 {
                    sym_tags.push(format!("{} {}", count.tests, plural(count.tests, "test")));
                }
                if count.fixtures > 0 {
                    sym_tags.push(format!(
                        "{} {}",
                        count.fixtures,
                        plural(count.fixtures, "fixture")
                    ));
                }
                if sym_tags.is_empty() {
                    curr.plain_files.push(comp.to_string());
                } else {
                    curr.files.insert(comp.to_string(), sym_tags);
                }
            }
        } else if i < max_depth {
            curr = curr.subdirs.entry(comp.to_string()).or_default();
        } else {
            curr.hidden_files += 1;
            break;
        }
    }
}

/// The most files with definitions a subfolder lists; the folder the outline starts at lists all.
const FILES_LISTED_PER_FOLDER: usize = 40;

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

    let listed = if depth == 0 {
        node.files.len()
    } else {
        node.files.len().min(FILES_LISTED_PER_FOLDER)
    };
    let more_files = node.files.len() - listed;
    let total_items = node.subdirs.len()
        + listed
        + usize::from(more_files > 0)
        + usize::from(!node.plain_files.is_empty());
    let mut index = 0;

    // Render subdirectories
    for (name, sub) in &node.subdirs {
        index += 1;
        let is_last = index == total_items;
        let branch = if is_last { "└── " } else { "├── " };
        let next_prefix = format!("{}{}", prefix, if is_last { "    " } else { "│   " });

        let hidden = match (sub.hidden_files, sub.unsupported_files) {
            (0, 0) => String::new(),
            (0, m) => format!(" ({m} unsupported {})", plural(m, "file")),
            (n, 0) => format!(" ({n} indexed {})", plural(n, "file")),
            (n, m) => format!(" ({n} indexed {}, {m} unsupported)", plural(n, "file")),
        };
        out.push_str(&format!("{prefix}{branch}{name}/{hidden}\n"));
        render_outline_tree(out, sub, &next_prefix, depth + 1, max_depth);
    }

    // Render files
    for (file_name, syms) in node.files.iter().take(listed) {
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

    if more_files > 0 {
        index += 1;
        let branch = if index == total_items {
            "└── "
        } else {
            "├── "
        };
        out.push_str(&format!(
            "{prefix}{branch}(+{more_files} more {} with functions or classes; pass this folder as the path to list them)\n",
            plural(more_files, "file")
        ));
    }

    if !node.plain_files.is_empty() {
        let count = node.plain_files.len();
        let names = if count <= 5 {
            node.plain_files.join(", ")
        } else {
            format!("{}, +{} more", node.plain_files[..3].join(", "), count - 3)
        };
        out.push_str(&format!(
            "{prefix}└── ({count} {} without functions or classes: {names})\n",
            plural(count, "file")
        ));
    }
}

/// Format symbol body with metadata header, signature, and body content.
/// A symbol's source as written, under a location comment.
pub fn format_symbol_body(symbol: &Symbol, source: &str) -> String {
    let mut out = format!(
        "// {}:{}-{} ({})\n{source}",
        symbol.path, symbol.start_line, symbol.end_line, symbol.name
    );
    if !source.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Format surgical context bundle for a symbol.
pub fn format_context_slice(slice: &ContextSlice) -> String {
    let sym = &slice.target_symbol;
    let mut out = String::new();
    out.push_str(&format!(
        "### Target: `{}` ({}:{}-{})\n\n",
        sym.name, sym.path, sym.start_line, sym.end_line
    ));

    if let Some(ref sig) = sym.signature {
        let sig = signature_without_duplicate_body(sig, &slice.target_body);
        if !sig.trim().is_empty() {
            out.push_str(&format!("Signature: `{sig}`\n\n"));
        }
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
        out.push_str("These tests call, use, or name this symbol; blast_radius also lists tests that can reach it through callers.\n\n");
    } else {
        out.push_str(
            "### Related Tests:\nNo test calls, uses, or names this symbol; blast_radius lists tests that can reach it through callers.\n",
        );
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

/// The FTS match snippet on one line, or `None` when it only repeats the signature. The comment
/// markers that open each doc-comment line (`//`, `///`, `//!`, `*`) are dropped when lines join.
fn match_line(snippet: &str, signature: &str) -> Option<String> {
    let line = snippet
        .lines()
        .enumerate()
        .map(|(i, text)| match i {
            0 => text.trim(),
            _ => text.trim().trim_start_matches(['/', '*', '!']).trim(),
        })
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    let plain = |text: &str| {
        text.split_whitespace()
            .collect::<String>()
            .replace(['[', ']'], "")
    };
    let matched = plain(line.trim_start_matches("...").trim_end_matches("..."));
    (!line.is_empty() && !plain(signature).contains(&matched)).then_some(line)
}

/// A callee name on one line. An unresolved call chain longer than 60 characters that holds a
/// call keeps only its last call, so a multi-line receiver expression never floods the list.
fn callee_name(name: &str) -> String {
    let line = name.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= 60 || !line.contains('(') {
        return line;
    }
    match line.rsplit_once('.') {
        Some((_, last)) if !last.is_empty() => format!("….{last}"),
        _ => line,
    }
}

/// Format references list for callers/callees with optional limit footer.
/// One line for the import statements of a target: their count and the first three sites.
pub fn format_import_summary(sites: &[(String, usize)]) -> String {
    if sites.is_empty() {
        return String::new();
    }
    let files: std::collections::BTreeSet<&str> =
        sites.iter().map(|(path, _)| path.as_str()).collect();
    let first: Vec<String> = sites
        .iter()
        .take(3)
        .map(|(path, line)| format!("{path}:{line}"))
        .collect();
    let more = if sites.len() > 3 { ", …" } else { "" };
    format!(
        "Imported {} {} in {} {}: {}{more} (lookup_symbol with kind=\"import\" lists them)\n",
        sites.len(),
        plural(sites.len(), "time"),
        files.len(),
        plural(files.len(), "file"),
        first.join(", ")
    )
}

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
            r.from_symbol_name.clone()
        } else {
            callee_name(&r.to_symbol_name)
        };
        let in_file = match r.occurrences {
            Some(n) => format!(", {n} in file"),
            None => String::new(),
        };
        let target = match &r.target {
            Some(target) if direction != "callers" => format!(" → {target}"),
            _ => String::new(),
        };
        out.push_str(&format!(
            "- `{other}` [{}{line_info}] (kind: {}{in_file}){target}\n",
            r.path, r.kind
        ));
    }

    if refs.len() >= limit {
        out.push_str(&cap_notice(refs.len(), limit));
    }

    out
}

/// Formats exact or FTS fallback symbol results with transparent header labeling.
/// One line for the lookup rows whose name only starts with or contains the query, shown when
/// some row is named exactly: each distinct name once, with its kind and count.
fn other_names_line(query: &str, others: &[&Symbol]) -> String {
    let mut groups: Vec<(String, &str, usize)> = Vec::new();
    for s in others {
        let kind = display_kind(s);
        match groups
            .iter_mut()
            .find(|(name, k, _)| *name == s.name && *k == kind)
        {
            Some(group) => group.2 += 1,
            None => groups.push((s.name.clone(), kind, 1)),
        }
    }
    let listed: Vec<String> = groups
        .iter()
        .take(8)
        .map(|(name, kind, count)| match count {
            1 => format!("`{name}` ({kind})"),
            n => format!("`{name}` ({kind}, {n}×)"),
        })
        .collect();
    let more = match groups.len().saturating_sub(8) {
        0 => String::new(),
        n => format!(", +{n} more"),
    };
    format!(
        "- {} other {} `{query}`, ignoring case: {}{more} (lookup_symbol with one of these names lists its rows)\n",
        others.len(),
        if others.len() == 1 {
            "row starts with or contains"
        } else {
            "rows start with or contain"
        },
        listed.join(", ")
    )
}

pub fn format_find_symbol_results(
    query: &str,
    exact_matches: &[Symbol],
    fts_matches: &[SymbolSearchResult],
    limit: usize,
) -> String {
    if !exact_matches.is_empty() {
        let named_exactly = exact_matches
            .iter()
            .filter(|s| s.name == query || s.name.ends_with(&format!(".{query}")))
            .count();
        let exact_note = if named_exactly < exact_matches.len() {
            format!(" ({named_exactly} named exactly `{query}`; the rest start with or contain it)")
        } else {
            String::new()
        };
        let owner = query
            .strip_suffix("::")
            .or_else(|| query.strip_suffix('.'))
            .filter(|owner| !owner.is_empty());
        let mut out = match owner {
            Some(owner) => format!("Found {} members of `{owner}`:\n\n", exact_matches.len()),
            None => format!(
                "Found {} symbols matching \"{query}\"{exact_note}:\n\n",
                exact_matches.len()
            ),
        };
        let has_definition = exact_matches.iter().any(|s| s.kind != "import");
        let folds = |s: &Symbol| crate::queries::folds_into_import_line(query, s, has_definition);
        let imports: Vec<&Symbol> = exact_matches.iter().filter(|s| folds(s)).collect();
        let is_exact = |s: &Symbol| s.name == query || s.name.ends_with(&format!(".{query}"));
        let (shown, others): (Vec<&Symbol>, Vec<&Symbol>) = exact_matches
            .iter()
            .filter(|s| !folds(s))
            .partition(|s| named_exactly == 0 || is_exact(s));
        for s in shown {
            let sig = s.signature.as_deref().unwrap_or(&s.name);
            out.push_str(&format!(
                "- {} `{}` [{}:{}-{}] id={}\n",
                display_kind(s),
                s.name,
                s.path,
                s.start_line,
                s.end_line,
                s.symbol_id
            ));
            out.push_str(&format!("  Signature: {sig}\n"));
            if let Some(doc) = &s.doc_comment {
                let first = doc.lines().next().unwrap_or("").trim();
                if !first.is_empty() {
                    out.push_str(&format!("  Doc: {first}\n"));
                }
            }
        }
        if !imports.is_empty() {
            let shown = imports
                .iter()
                .take(3)
                .map(|s| format!("{}:{}", s.path, s.start_line))
                .collect::<Vec<_>>()
                .join(", ");
            let more = if imports.len() > 3 { ", …" } else { "" };
            let cut_inside_exact_names = exact_matches
                .last()
                .is_some_and(|s| s.name.eq_ignore_ascii_case(query) || folds(s));
            let capped = if exact_matches.len() - imports.len() >= limit && cut_inside_exact_names {
                "at least "
            } else {
                ""
            };
            out.push_str(&format!(
                "- {capped}{} {} of `{query}`: {shown}{more} (lookup_symbol with kind=\"import\" lists them)\n",
                imports.len(),
                plural(imports.len(), "import")
            ));
        }
        if !others.is_empty() {
            out.push_str(&other_names_line(query, &others));
        }
        if exact_matches.len() - imports.len() >= limit {
            out.push_str(&cap_notice(exact_matches.len() - imports.len(), limit));
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
                "- {} `{}` [{}:{}-{}] (score: {:.2}) id={}\n",
                display_kind(s),
                s.name,
                s.path,
                s.start_line,
                s.end_line,
                r.score,
                s.symbol_id
            ));
            out.push_str(&format!("  Signature: {sig}\n"));
            if let Some(line) = r.snippet.as_deref().and_then(|m| match_line(m, sig)) {
                out.push_str(&format!("  Match: {line}\n"));
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
/// The first line of a fact query that matched nothing; it names the path the query was limited to.
pub fn no_facts_heading(category: &str, path_filter: Option<&str>) -> String {
    match path_filter {
        Some(path) => format!("No facts match '{category}' under `{path}`."),
        None => format!("No facts match '{category}' in this repository."),
    }
}

/// The answer when a category matches no fact. A known alias gets only the alias summary, and
/// `import` says where imports are, because most extractors record imports as symbols. Any other
/// category lists every raw category, so the agent can pick one.
pub fn format_no_facts(
    category: &str,
    path_filter: Option<&str>,
    categories: &[(String, usize)],
) -> String {
    let mut out = format!("{}\n\n", no_facts_heading(category, path_filter));
    if !crate::queries::is_category_alias(category) {
        out.push_str(&format_fact_categories(categories));
        return out;
    }
    if matches!(category.to_ascii_lowercase().as_str(), "import" | "imports") {
        out.push_str("Most languages record imports as symbols: lookup_symbol with kind=\"import\" lists them.\n\n");
    }
    out.push_str(&alias_summary(categories));
    out
}

fn alias_summary(categories: &[(String, usize)]) -> String {
    let aliases = crate::queries::alias_fact_counts(categories);
    if aliases.is_empty() {
        return String::new();
    }
    let parts: Vec<String> = aliases
        .iter()
        .map(|(alias, patterns, facts)| {
            format!(
                "{alias} ({patterns} {}, {facts} {})",
                plural(*patterns, "pattern"),
                plural(*facts, "fact")
            )
        })
        .collect();
    format!("Aliases: {}\n\n", parts.join(", "))
}

pub fn format_fact_categories(categories: &[(String, usize)]) -> String {
    if categories.is_empty() {
        return "No structural facts or literals indexed in this repository.".to_string();
    }
    let mut out = String::new();
    out.push_str(&alias_summary(categories));
    // A decorated-definition fact only records where a decorated block starts.
    let (patterns, literal_kinds): (Vec<_>, Vec<_>) = categories
        .iter()
        .filter(|(name, _)| !name.ends_with(".decorated_definition.v1"))
        .partition(|(name, _)| name.contains('.'));
    out.push_str(&format!(
        "Available structural fact categories ({} found):\n\n",
        patterns.len()
    ));
    for (name, count) in &patterns {
        out.push_str(&format!(
            "- `{name}` ({count} {})\n",
            plural(*count, "fact")
        ));
    }
    if !literal_kinds.is_empty() {
        out.push_str("\nString literal kinds (the same command lists the literals of a kind):\n\n");
        for (name, count) in &literal_kinds {
            out.push_str(&format!(
                "- `{name}` ({count} {})\n",
                plural(*count, "literal")
            ));
        }
    }
    out
}

fn plural(count: usize, word: &str) -> String {
    if count == 1 {
        word.to_string()
    } else {
        format!("{word}s")
    }
}

/// Format structural facts and matching literals into token-dense markdown.
pub fn format_structural_facts(
    facts: &[crate::models::StructuralFact],
    literals: &[crate::models::LiteralFact],
    category: &str,
    limit: usize,
) -> String {
    let mut out = format!(
        "Structural facts for '{category}' ({} found):\n",
        facts.len()
    );
    for f in facts {
        let label = f.key.as_deref().unwrap_or(&f.capture_name);
        let details = qt_property_details(f);
        let api_style = f
            .metadata
            .as_ref()
            .and_then(|m| m.get("api_style"))
            .and_then(|v| v.as_str());
        let place = match api_style {
            Some("decorator_routing") => "handler",
            Some("call_routing") => "registered in",
            _ => "in",
        };
        let parent = f
            .containing_symbol_name
            .as_deref()
            .map(|p| format!(", {place}: {p}"))
            .unwrap_or_default();
        out.push_str(&format!(
            "- {label} [{}:{}] (pattern: {}{details}{parent})\n",
            f.path, f.start_line, f.pattern_id
        ));
    }
    if limit > 0 && facts.len() >= limit {
        out.push_str(&cap_notice(facts.len(), limit));
    }
    if facts
        .iter()
        .any(|f| f.pattern_id.starts_with("flask.route"))
    {
        out.push_str("Routes as declared in the source. By default Flask also answers HEAD for GET and OPTIONS for every rule, and serves `/static/<path:filename>`; the app or a rule can turn these off.\n");
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
        if limit > 0 && literals.len() >= limit {
            out.push_str(&cap_notice(literals.len(), limit));
        }
    }
    out
}

fn qt_property_details(fact: &crate::models::StructuralFact) -> String {
    if fact.pattern_id != "cpp.qt_property.v1" {
        return String::new();
    }
    let Some(metadata) = fact.metadata.as_ref() else {
        return String::new();
    };
    [
        "property_type",
        "read",
        "write",
        "notify",
        "designable",
        "scriptable",
        "stored",
        "user",
        "revision",
    ]
    .into_iter()
    .filter_map(|key| metadata.get(key).map(|value| (key, value)))
    .map(|(key, value)| {
        let value = value
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string());
        format!(", {key}: {value}")
    })
    .collect()
}

/// Formats FTS5 conceptual search results into token-dense markdown.
pub fn format_search_results(query: &str, results: &[SymbolSearchResult], limit: usize) -> String {
    if results.is_empty() {
        return format!("No symbols found matching concept \"{query}\".");
    }

    let mut out = format!(
        "Found {} symbols matching concept \"{query}\":\n",
        results.len()
    );
    if let Some(explain) = results.first().and_then(|r| r.explain.as_ref()) {
        out.push_str(&format!(
            "rerank: {} candidates in {} µs",
            explain.candidates, explain.rerank_us
        ));
        if !explain.word_weights.is_empty() {
            let words: Vec<String> = explain
                .word_weights
                .iter()
                .map(|(word, weight)| format!("{word} {weight:.2}"))
                .collect();
            out.push_str(&format!("; words {}", words.join(", ")));
        }
        out.push('\n');
    }
    out.push('\n');
    for r in results {
        let s = &r.symbol;
        let sig = s.signature.as_deref().unwrap_or(&s.name);
        out.push_str(&format!(
            "- {} `{}` [{}:{}-{}] (score: {:.2}) id={}\n",
            display_kind(s),
            s.name,
            s.path,
            s.start_line,
            s.end_line,
            r.score,
            s.symbol_id
        ));
        out.push_str(&format!("  Signature: {sig}\n"));
        if let Some(line) = r.snippet.as_deref().and_then(|m| match_line(m, sig)) {
            out.push_str(&format!("  Match: {line}\n"));
        } else if let Some(doc) = &s.doc_comment {
            let first_line = doc.lines().next().unwrap_or("").trim();
            if !first_line.is_empty() {
                out.push_str(&format!("  Doc: {first_line}\n"));
            }
        }
        if let Some(explain) = &r.explain {
            out.push_str(&format!("  explain: {}\n", explain_line(r.score, explain)));
        }
    }

    if results.len() >= limit {
        out.push_str(&cap_notice(results.len(), limit));
    }

    out
}

fn explain_line(score: f64, e: &SearchExplain) -> String {
    let mut line = format!(
        "score {score:.1} = terms {:.1} + name {}({}) {:.1} + kind {:.1} + path {:.1}",
        e.term_score, e.name_tier, e.name_strength, e.name_bonus, e.kind_prior, e.path_role,
    );
    if e.documentation != 0.0 {
        line.push_str(&format!(" + doc {:.1}", e.documentation));
    }
    if e.test_intent != 0.0 {
        line.push_str(&format!(" + test {:.1}", e.test_intent));
    }
    if e.nested != 0.0 {
        line.push_str(&format!(" + nested {:.1}", e.nested));
    }
    line.push_str(&format!(" [{}]", e.branches.join(",")));
    if let Some(bm25) = e.bm25 {
        line.push_str(&format!(" bm25 {bm25:.2}"));
    }
    if !e.terms.is_empty() {
        let terms: Vec<String> = e
            .terms
            .iter()
            .map(|(term, field, credit)| format!("{term}={field}:{credit}"))
            .collect();
        line.push_str(&format!(" terms {}", terms.join(" ")));
    }
    line
}

/// Format blast radius and likely test targets into token-dense markdown.
pub fn format_blast_radius(result: &BlastRadiusResult) -> String {
    if result.seed_type == "none" {
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

    let more = if result.limit_at_maximum {
        "limit is at its maximum, so narrow the target to a symbol or fewer files"
    } else {
        "raise limit to see the rest"
    };
    let shown_of = |shown: usize, found: usize, noun: &str| {
        if found > shown {
            format!("Showing {shown} of {found} {noun}; {more}.\n\n")
        } else {
            format!("Requested limit hid additional {noun}; {more}.\n\n")
        }
    };
    if result.likely_tests_truncated {
        out.push_str(&shown_of(
            result.likely_tests.len(),
            result.likely_tests_found,
            "likely tests",
        ));
    }
    if result.impacted_symbols_truncated {
        out.push_str(&shown_of(
            result.impacted_symbols.len(),
            result.impacted_symbols_found,
            "impacted symbols",
        ));
    }
    if result.traversal_ceiling_reached {
        out.push_str("Traversal stopped at the 200-row discovery ceiling; narrow the target because increasing limit cannot raise this ceiling.\n\n");
    }
    if result.test_file_ceiling_reached {
        out.push_str("Name-matched test discovery hit its fixed ceiling; narrow the target because increasing limit cannot raise it.\n\n");
    }

    if !result.likely_tests.is_empty() {
        let total = result.likely_tests.len();
        out.push_str(&format!("### Likely Tests to Run ({} returned)\n", total));

        let mut tests_by_file: std::collections::BTreeMap<&str, Vec<&TestTarget>> =
            std::collections::BTreeMap::new();
        let mut file_order = Vec::new();
        for t in &result.likely_tests {
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

        if result
            .likely_tests
            .iter()
            .any(|t| t.reason.starts_with("possible:"))
        {
            out.push_str("`possible` rows build the class and use a test client, so they reach the target only through a runtime call to `__call__`. The index picks them by shared name words and cannot see whether they send a call that reaches the target; run the whole suite for full coverage.\n");
        }
        out.push('\n');
    } else if result.likely_tests_truncated
        || result.traversal_ceiling_reached
        || result.test_file_ceiling_reached
    {
        out.push_str("### Likely Tests to Run (0 returned)\n\n");
    } else {
        out.push_str(
            "### Likely Tests to Run (0 returned)\nNo direct or name-matched tests found.\n\n",
        );
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

        out.push_str(&format!("### Downstream Impact ({} returned)\n", total));

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
            let mut syms_by_file: std::collections::BTreeMap<&str, Vec<&ImpactedSymbol>> =
                std::collections::BTreeMap::new();
            let mut file_order = Vec::new();
            for s in &visible {
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
    } else if result.impacted_symbols_truncated || result.traversal_ceiling_reached {
        out.push_str("### Downstream Impact (0 returned)\n");
    } else {
        out.push_str(
            "### Downstream Impact (0 returned)\nNo downstream callers found within depth.\n",
        );
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
            metadata: None,
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
            30,
        );
        assert!(with_key.contains(
            "- mcp_servers.code-kb.command [.codex/config.toml:2] (pattern: toml.key_value.v1)"
        ));

        let without_key = format_structural_facts(&[structural_fact(None)], &[], "config", 30);
        assert!(
            without_key.contains("- key_value [.codex/config.toml:2] (pattern: toml.key_value.v1)")
        );
    }

    #[test]
    fn format_structural_facts_renders_selected_qt_property_metadata() {
        let mut fact = structural_fact(Some("index"));
        fact.pattern_id = "cpp.qt_property.v1".into();
        fact.metadata = Some(serde_json::json!({
            "property_type": "int",
            "designable": false,
            "scriptable": true,
            "stored": false,
            "user": true,
            "revision": 2,
        }));

        let output = format_structural_facts(&[fact], &[], "property", 30);

        assert!(output.contains(
            "property_type: int, designable: false, scriptable: true, stored: false, user: true, revision: 2"
        ));
    }

    #[test]
    fn format_references_reports_the_occurrence_count_of_a_grouped_row() {
        let site = |occurrences| ReferenceSite {
            from_symbol_name: "Button".into(),
            from_symbol_id: "s1".into(),
            to_symbol_name: "background".into(),
            kind: "member_access".into(),
            path: "Ui/Button.qml".into(),
            start_line: Some(5),
            start_column: Some(4),
            occurrences,
            target: None,
        };

        let grouped = format_references("Color", &[site(Some(6))], "callers", 30);
        assert!(
            grouped.contains("- `Button` [Ui/Button.qml:5] (kind: member_access, 6 in file)"),
            "{grouped}"
        );

        let single = format_references("Color", &[site(None)], "callers", 30);
        assert!(
            single.contains("- `Button` [Ui/Button.qml:5] (kind: member_access)"),
            "{single}"
        );
    }

    #[test]
    fn a_match_snippet_joins_its_lines_and_is_dropped_when_it_repeats_the_signature() {
        assert_eq!(
            match_line("...path up to\n the [root] (the [root] excluded)", "fn f()"),
            Some("...path up to the [root] (the [root] excluded)".into())
        );
        assert_eq!(
            match_line("// [HelpFunc] returns the\n// [function] set", "fn f()"),
            Some("// [HelpFunc] returns the [function] set".into())
        );
        assert_eq!(
            match_line(
                "fn [index]_facts(workspace_[root]: &Path) -> Option<[IndexFacts]>",
                "fn index_facts(workspace_root: &Path) -> Option<IndexFacts>"
            ),
            None
        );
        assert_eq!(
            match_line(
                "...[root]: &[PathBuf]) -> bool",
                "fn f(root: &[PathBuf]) -> bool"
            ),
            None
        );
    }

    #[test]
    fn a_long_unresolved_call_chain_shows_only_its_last_call() {
        assert_eq!(
            callee_name(
                "[\"a\", \"b\"]\n    .into_iter()\n    .find_map(|key| args.get(key)).ok_or_else"
            ),
            "….ok_or_else"
        );
        assert_eq!(
            callee_name("workspace.resolve_path"),
            "workspace.resolve_path"
        );
        let selector = "html.dash .header, html.dash .breadcrumbs, html.dash .navigation";
        assert_eq!(callee_name(selector), selector);
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
    fn format_symbol_body_prints_the_source_under_its_location() {
        let mut symbol = sample_symbol("plain");
        symbol.signature = Some("pub fn plain()".into());

        assert_eq!(
            format_symbol_body(&symbol, "pub fn plain() {\n    1\n}"),
            "// src/lib.rs:1-10 (plain)\npub fn plain() {\n    1\n}\n"
        );
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
            explain: None,
        }];

        let formatted = format_search_results("tokens", &results, 20);
        assert!(formatted.contains("Found 1 symbols matching concept \"tokens\":\n\n- "));
        assert!(
            formatted.contains("- function `parse_tokens` [src/parser.rs:15-25] (score: -1.85)")
        );
        assert!(formatted.contains("Match: Parses [tokens] from stream."));
        assert!(!formatted.contains("explain"));
        assert!(!formatted.contains("rerank"));
    }

    #[test]
    fn exact_lookup_folds_import_rows_into_one_line_when_a_definition_exists() {
        let class = Symbol {
            kind: "class".into(),
            path: "src/flask/app.py".into(),
            ..sample_symbol("Flask")
        };
        let import = |path: &str, line: usize| Symbol {
            kind: "import".into(),
            path: path.into(),
            start_line: line,
            symbol_id: format!("id_{path}"),
            ..sample_symbol("Flask")
        };
        let rows = vec![
            class,
            import("src/flask/cli.py", 34),
            import("src/flask/ctx.py", 21),
            import("tests/conftest.py", 6),
            import("examples/app.py", 1),
        ];

        let folded = format_find_symbol_results("Flask", &rows, &[], 20);

        assert!(
            folded.contains("- class `Flask` [src/flask/app.py:"),
            "{folded}"
        );
        assert!(
            folded.contains(
                "- 4 imports of `Flask`: src/flask/cli.py:34, src/flask/ctx.py:21, tests/conftest.py:6, … (lookup_symbol with kind=\"import\" lists them)"
            ),
            "{folded}"
        );
        assert!(!folded.contains("- import `Flask`"), "{folded}");

        let capped = format_find_symbol_results("Flask", &rows, &[], 1);
        assert!(
            capped.contains("- at least 4 imports of `Flask`"),
            "{capped}"
        );

        let mut with_prefix_rows = rows.clone();
        with_prefix_rows.push(Symbol {
            kind: "class".into(),
            ..sample_symbol("FlaskGroup")
        });
        let cut_after_exact = format_find_symbol_results("Flask", &with_prefix_rows, &[], 6);
        assert!(
            cut_after_exact.contains("- 4 imports of `Flask`"),
            "{cut_after_exact}"
        );

        let imports_only = format_find_symbol_results("Flask", &rows[1..], &[], 20);
        assert_eq!(imports_only.matches("- import `Flask`").count(), 4);
    }

    #[test]
    fn skeleton_shows_python_class_and_module_variables_with_one_line_values() {
        let leaf = |name: &str, kind: &str, line: usize, sig: &str| Symbol {
            kind: kind.into(),
            language: "python".into(),
            path: "app.py".into(),
            parent_symbol_id: Some("id_App".into()),
            start_line: line,
            end_line: line,
            body_start_line: None,
            body_end_line: None,
            signature: Some(sig.into()),
            ..sample_symbol(name)
        };
        let class = Symbol {
            kind: "class".into(),
            language: "python".into(),
            path: "app.py".into(),
            start_line: 1,
            end_line: 20,
            body_start_line: None,
            body_end_line: None,
            signature: Some("class App".into()),
            ..sample_symbol("App")
        };
        let module_variable = Symbol {
            kind: "variable".into(),
            language: "python".into(),
            path: "app.py".into(),
            start_line: 30,
            end_line: 30,
            body_start_line: None,
            body_end_line: None,
            signature: Some("app = App()".into()),
            ..sample_symbol("app")
        };
        let symbols = vec![
            class,
            leaf(
                "request_class",
                "variable",
                2,
                "request_class: type[Request] = Request",
            ),
            leaf(
                "default_config",
                "variable",
                3,
                "default_config = ImmutableDict(\n    {\n        \"DEBUG\": None,\n    }\n)",
            ),
            leaf(
                "blueprints",
                "property",
                5,
                "self.blueprints: dict[str, Blueprint] = {}",
            ),
            leaf(
                "error_handler_spec",
                "property",
                7,
                "self.error_handler_spec: dict[\n    ft.AppOrBlueprintKey,\n    dict[int | None, dict[type[Exception], ft.ErrorHandlerCallable]],\n] = defaultdict(lambda: defaultdict(dict), default_factory_argument_that_is_long)  # type: ignore",
            ),
            module_variable,
        ];

        let out = format_file_skeleton("app.py", &symbols, None, 0);

        assert!(
            out.contains("    request_class: type[Request] = Request; // L2-2"),
            "{out}"
        );
        assert!(
            out.contains("    default_config = ImmutableDict({\"DEBUG\": None}); // L3-3"),
            "{out}"
        );
        assert!(
            out.contains("    self.error_handler_spec: dict[ft.AppOrBlueprintKey, dict[int | None, dict[type[Exception], ft.ErrorHandlerCallable]]] = defaultdict(lambda: defaultdict(dict)…; // L7-7"),
            "{out}"
        );
        assert!(
            out.contains("    self.blueprints: dict[str, Blueprint] = {}; // L5-5"),
            "{out}"
        );
        assert!(out.contains("app = App(); // L30-30"), "{out}");
    }

    #[test]
    fn skeleton_names_the_method_that_assigns_an_attribute() {
        let member = |name: &str, kind: &str, start: usize, end: usize, sig: &str| Symbol {
            kind: kind.into(),
            language: "python".into(),
            parent_symbol_id: Some("id_Flask".into()),
            start_line: start,
            end_line: end,
            body_start_line: None,
            body_end_line: None,
            signature: Some(sig.into()),
            ..sample_symbol(name)
        };
        let class = Symbol {
            kind: "class".into(),
            language: "python".into(),
            start_line: 1,
            end_line: 30,
            body_start_line: None,
            body_end_line: None,
            signature: Some("class Flask(App)".into()),
            ..sample_symbol("Flask")
        };
        let symbols = vec![
            class,
            member("__init__", "method", 2, 10, "def __init__(self)"),
            member("cli", "property", 3, 3, "self.cli = cli.AppGroup()"),
            member("run", "method", 12, 20, "def run(self)"),
            member("debug", "property", 14, 14, "self.debug = get_debug_flag()"),
        ];

        let out = format_file_skeleton("app.py", &symbols, None, 0);

        assert!(
            out.contains("self.cli = cli.AppGroup(); // in __init__(), L3-3"),
            "{out}"
        );
        assert!(
            out.contains("self.debug = get_debug_flag(); // in run(), L14-14"),
            "{out}"
        );
    }

    #[test]
    fn skeleton_hides_variables_that_are_not_python_class_attributes() {
        let parent = |name: &str, kind: &str, language: &str| Symbol {
            kind: kind.into(),
            language: language.into(),
            start_line: 1,
            end_line: 20,
            body_start_line: None,
            body_end_line: None,
            signature: Some(format!("{kind} {name}")),
            ..sample_symbol(name)
        };
        let child = |name: &str, parent: &str, language: &str, sig: &str| Symbol {
            kind: "variable".into(),
            language: language.into(),
            parent_symbol_id: Some(format!("id_{parent}")),
            start_line: 2,
            end_line: 2,
            body_start_line: None,
            body_end_line: None,
            signature: Some(sig.into()),
            ..sample_symbol(name)
        };
        let symbols = vec![
            parent("Adapters", "class", "java"),
            child(
                "accessible",
                "Adapters",
                "java",
                "boolean accessible = false",
            ),
            parent("body", "class", "html"),
            child(
                "img",
                "body",
                "html",
                "<img class=\"carat\" src=\"carat.png\" alt>",
            ),
            parent("section", "module", "yaml"),
            child("title", "section", "yaml", "title: Invoking jq"),
        ];

        let out = format_file_skeleton("mixed", &symbols, None, 0);

        for hidden in ["accessible", "<img", "title:"] {
            assert!(!out.contains(hidden), "{out}");
        }
    }

    #[test]
    fn a_multi_line_macro_row_drops_its_line_continuations_and_is_cut_to_one_line() {
        let short = "#define PAIR(a, b)   \\\n    a,                \\\n    b";
        assert_eq!(value_row_signature(short), "#define PAIR(a, b) a, b");

        let long = format!(
            "#define WIDE(x) \\\n{}",
            "    x + x + x + x + x + x + x + x \\\n".repeat(8)
        );
        let row = value_row_signature(&long);
        assert!(row.starts_with("#define WIDE(x) x + x"), "{row}");
        assert!(row.ends_with('…'), "{row}");
        assert!(!row.contains('\\'), "{row}");
        assert!(row.chars().count() <= 120, "{row}");
    }

    #[test]
    fn skeleton_keeps_values_only_for_real_assignments() {
        let row =
            |name: &str, kind: &str, line: usize, sig: &str, body: Option<(usize, usize)>| Symbol {
                kind: kind.into(),
                start_line: line,
                end_line: body.map_or(line, |(_, end)| end),
                body_start_line: body.map(|(start, _)| start),
                body_end_line: body.map(|(_, end)| end),
                signature: Some(sig.into()),
                ..sample_symbol(name)
            };
        let symbols = vec![
            row(
                "img",
                "property",
                1,
                "[class^=rz-] img,[class^=rz-] svg { vertical-align:middle }",
                None,
            ),
            row(
                "Coordinates",
                "property",
                2,
                "[JsonProperty(ItemConverterType = typeof(IntToFloatConverter))] public int[,,] Coordinates { get; set; }",
                None,
            ),
            row(
                "VERSION_CHECK",
                "constant",
                3,
                "#define VERSION_CHECK(major,minor,patch) (_MSC_VER >= ((major * 100) + (minor)))\n",
                None,
            ),
            row(
                "TABLE",
                "constant",
                4,
                "TABLE = {\n  '\"' => '%22',\n  '\\r' => '%0D',\n}.freeze",
                Some((4, 8)),
            ),
            row(
                "LIMIT",
                "field",
                10,
                "private static final int LIMIT = 5;",
                None,
            ),
        ];

        let out = format_file_skeleton("mixed", &symbols, None, 0);

        assert!(
            out.contains("[class^=rz-] img,[class^=rz-] svg; // L1-1"),
            "{out}"
        );
        assert!(out.contains("public int[,,] Coordinates; // L2-2"), "{out}");
        assert!(
            out.contains(
                "#define VERSION_CHECK(major,minor,patch) (_MSC_VER >= ((major * 100) + (minor))); // L3-3\n"
            ),
            "{out}"
        );
        assert!(
            out.contains("TABLE = { /* 5 lines hidden: L4-L8 */ }"),
            "{out}"
        );
        assert!(
            out.contains("private static final int LIMIT = 5; // L10-10"),
            "{out}"
        );
    }

    #[test]
    fn skeleton_cuts_a_field_with_members_at_its_brace() {
        let field = Symbol {
            kind: "field".into(),
            language: "java".into(),
            start_line: 1,
            end_line: 9,
            body_start_line: None,
            body_end_line: None,
            signature: Some(
                "public static final TypeAdapter<Class> CLASS = new TypeAdapter<Class>() {\n  @Override\n}"
                    .into(),
            ),
            ..sample_symbol("CLASS")
        };
        let member = Symbol {
            kind: "method".into(),
            language: "java".into(),
            parent_symbol_id: Some("id_CLASS".into()),
            start_line: 2,
            end_line: 4,
            signature: Some("public void write(JsonWriter out, Class value)".into()),
            ..sample_symbol("write")
        };

        let out = format_file_skeleton("TypeAdapters.java", &[field, member], None, 0);

        assert!(
            out.contains(
                "public static final TypeAdapter<Class> CLASS = new TypeAdapter<Class>() {\n"
            ),
            "{out}"
        );
        assert!(!out.contains('…'), "{out}");
    }

    #[test]
    fn import_summary_counts_sites_and_files_and_lists_three() {
        let sites = vec![
            ("src/flask/__init__.py".to_string(), 2),
            ("src/flask/cli.py".to_string(), 34),
            ("src/flask/cli.py".to_string(), 45),
            ("tests/conftest.py".to_string(), 6),
        ];

        assert_eq!(
            format_import_summary(&sites),
            "Imported 4 times in 3 files: src/flask/__init__.py:2, src/flask/cli.py:34, src/flask/cli.py:45, … (lookup_symbol with kind=\"import\" lists them)\n"
        );
        assert_eq!(format_import_summary(&[]), "");
    }

    #[test]
    fn no_facts_for_an_alias_points_imports_at_lookup_and_skips_the_raw_list() {
        let categories = vec![("flask.route.v1".to_string(), 3)];

        let imports = format_no_facts("import", None, &categories);
        let unknown = format_no_facts("widgets", None, &categories);

        assert!(
            imports.contains("lookup_symbol with kind=\"import\""),
            "{imports}"
        );
        assert!(!imports.contains("`flask.route.v1`"), "{imports}");
        assert!(unknown.contains("`flask.route.v1` (3 facts)"), "{unknown}");
    }

    #[test]
    fn markup_and_data_rows_show_their_own_kind_words() {
        let row = |language: &str, kind: &str| Symbol {
            language: language.into(),
            kind: kind.into(),
            ..sample_symbol("x")
        };
        assert_eq!(display_kind(&row("html", "class")), "element");
        assert_eq!(display_kind(&row("sql", "class")), "table");
        assert_eq!(display_kind(&row("markdown", "module")), "section");
        assert_eq!(display_kind(&row("markdown", "import")), "link");
        let attribute = Symbol {
            kind: "property".into(),
            signature: Some("self.extensions = {}".into()),
            ..sample_symbol("extensions")
        };
        assert_eq!(display_kind(&attribute), "attribute");
        let fsharp_member = Symbol {
            language: "fsharp".into(),
            signature: Some("this.Total = decimal this.Qty * this.Price".into()),
            ..attribute
        };
        assert_eq!(display_kind(&fsharp_member), "property");
        assert_eq!(display_kind(&row("python", "class")), "class");
    }

    #[test]
    fn no_facts_heading_names_the_path_filter() {
        assert_eq!(
            no_facts_heading("config", Some("src/flask")),
            "No facts match 'config' under `src/flask`."
        );
        assert_eq!(
            no_facts_heading("config", None),
            "No facts match 'config' in this repository."
        );
    }

    #[test]
    fn outline_says_how_many_definitions_it_left_out_and_counts_tests() {
        let mut root = OutlineNode::default();
        let symbol = |name: &str, is_test: bool| Symbol {
            kind: "function".into(),
            path: "tests/test_basic.py".into(),
            is_test,
            ..sample_symbol(name)
        };
        let symbols = HashMap::from([(
            "tests/test_basic.py".to_string(),
            vec![symbol("helper", false), symbol("test_one", true)],
        )]);
        let counts = HashMap::from([(
            "tests/test_basic.py".to_string(),
            crate::queries::OutlineCounts {
                definitions: 3,
                tests: 90,
                fixtures: 2,
            },
        )]);
        add_path_to_outline(&mut root, "tests/test_basic.py", &symbols, &counts, 3, "");

        let mut out = String::new();
        render_outline_tree(&mut out, &root, "", 0, 3);

        assert!(
            out.contains("test_basic.py [function helper, +2 more, 90 tests, 2 fixtures]"),
            "{out}"
        );
    }

    #[test]
    fn outline_counts_the_files_below_the_depth_limit() {
        let mut root = OutlineNode::default();
        let no_symbols = HashMap::new();
        for path in [
            "src/flask/app.py",
            "src/flask/json/tag.py",
            "src/flask/cli.py",
            "tests/conftest.py",
        ] {
            add_path_to_outline(&mut root, path, &no_symbols, &HashMap::new(), 2, "");
        }

        let mut out = String::new();
        render_outline_tree(&mut out, &root, "", 0, 2);

        assert!(out.contains("└── flask/ (3 indexed files)"), "{out}");
        assert!(!out.contains("tests/ ("), "{out}");
    }

    #[test]
    fn exact_lookup_folds_only_imports_named_exactly_like_the_query() {
        let rows = vec![
            Symbol {
                kind: "function".into(),
                ..sample_symbol("read")
            },
            Symbol {
                kind: "import".into(),
                path: "src/flask/cli.py".into(),
                start_line: 1034,
                ..sample_symbol("readline")
            },
            Symbol {
                kind: "import".into(),
                path: "src/Types.kt".into(),
                start_line: 18,
                ..sample_symbol("com.example.internal.read")
            },
        ];

        let out = format_find_symbol_results("read", &rows, &[], 20);

        assert!(
            out.contains(
                "- 1 other row starts with or contains `read`, ignoring case: `readline` (import) (lookup_symbol"
            ),
            "{out}"
        );
        assert!(
            out.contains(
                "- 1 import of `read`: src/Types.kt:18 (lookup_symbol with kind=\"import\" lists them)"
            ),
            "{out}"
        );
    }

    #[test]
    fn discovery_formatters_append_ids_to_metadata_rows() {
        let exact = vec![sample_symbol("exact")];
        let fallback = vec![SymbolSearchResult {
            symbol: sample_symbol("fallback"),
            score: 1.0,
            snippet: None,
            explain: None,
        }];

        let outputs = [
            (
                format_find_symbol_results("exact", &exact, &[], 20),
                "id_exact",
            ),
            (
                format_find_symbol_results("fallback query", &[], &fallback, 20),
                "id_fallback",
            ),
            (
                format_search_results("fallback", &fallback, 20),
                "id_fallback",
            ),
        ];

        for (formatted, id) in outputs {
            assert!(
                formatted
                    .lines()
                    .any(|line| line.starts_with("- ") && line.contains(&format!("id={id}"))),
                "{formatted}"
            );
            assert!(!formatted.contains("\n  id="), "{formatted}");
        }
    }

    #[test]
    fn every_printed_part_of_the_explain_line_sums_to_the_score() {
        let e = SearchExplain {
            bm25: Some(-3.21),
            branches: vec!["word".into()],
            name_tier: "all".into(),
            name_strength: 6,
            term_score: 24.5,
            name_bonus: 60.0,
            kind_prior: 4.0,
            path_role: -10.0,
            documentation: -200.0,
            test_intent: 5.0,
            nested: 0.0,
            terms: vec![("sha".into(), "name".into(), 3.0)],
            word_weights: vec![("sha".into(), 2.6)],
            candidates: 1,
            rerank_us: 1,
        };
        let score = e.term_score
            + e.name_bonus
            + e.kind_prior
            + e.path_role
            + e.documentation
            + e.test_intent;

        let line = explain_line(score, &e);
        let parts: f64 = line
            .split(" = ")
            .nth(1)
            .unwrap()
            .split(" [")
            .next()
            .unwrap()
            .split(" + ")
            .map(|part| part.rsplit(' ').next().unwrap().parse::<f64>().unwrap())
            .sum();

        assert!(line.starts_with(&format!(
            "score {score:.1} = terms 24.5 + name all(6) 60.0 "
        )));
        assert_eq!(format!("{parts:.1}"), format!("{score:.1}"));
    }

    #[test]
    fn test_format_search_results_prints_the_explain_breakdown_when_present() {
        let mut result = SymbolSearchResult {
            symbol: sample_symbol("parseSha256Sidecar"),
            score: 71.6,
            snippet: Some("parse[Sha256]Sidecar".into()),
            explain: Some(SearchExplain {
                bm25: Some(-3.21),
                branches: vec!["word".into(), "name".into()],
                name_tier: "all".into(),
                name_strength: 6,
                terms: vec![
                    ("sha".into(), "name".into(), 3.0),
                    ("256".into(), "name".into(), 3.0),
                ],
                term_score: 12.6,
                name_bonus: 60.0,
                kind_prior: 4.0,
                path_role: -10.0,
                documentation: 0.0,
                test_intent: 5.0,
                nested: 0.0,
                word_weights: vec![("sha".into(), 2.6), ("256".into(), 0.97)],
                candidates: 37,
                rerank_us: 180,
            }),
        };

        let formatted = format_search_results("sha256", std::slice::from_ref(&result), 20);
        assert!(formatted.contains(
            "Found 1 symbols matching concept \"sha256\":\nrerank: 37 candidates in 180 µs; words sha 2.60, 256 0.97\n\n- "
        ));
        assert!(formatted.contains(
            "  explain: score 71.6 = terms 12.6 + name all(6) 60.0 + kind 4.0 + path -10.0 + test 5.0 [word,name] bm25 -3.21 terms sha=name:3 256=name:3\n"
        ));

        result.explain = None;
        let silent = format_search_results("sha256", std::slice::from_ref(&result), 20);
        assert!(!silent.contains("explain"));
        assert!(!silent.contains("rerank"));
    }

    #[test]
    fn test_format_search_results_discloses_a_reached_limit() {
        let results = vec![SymbolSearchResult {
            symbol: sample_symbol("parse_tokens"),
            score: 0.0,
            snippet: None,
            explain: None,
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
                explain: None,
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
                reason: "direct caller".into(),
            }],
            impacted_symbols: vec![ImpactedSymbol {
                name: "caller_fn".into(),
                kind: "function".into(),
                path: "src/caller.rs".into(),
                line: 42,
                depth: 1,
            }],
            traversal_ceiling_reached: false,
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            test_file_ceiling_reached: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("## Blast Radius & Test Impact (Symbol: do_work)"));
        assert!(formatted.contains("### Likely Tests to Run (1 returned)"));
        assert!(
            formatted.contains("tests/work_test.rs:\n  - `test_do_work` [line 15] (direct caller)")
        );
        assert!(formatted.contains("src/caller.rs:\n  - [depth 1] function `caller_fn` [line 42]"));
    }

    #[test]
    fn format_blast_radius_groups_every_returned_test_by_file() {
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
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            test_file_ceiling_reached: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
        };

        let formatted = format_blast_radius(&res);

        assert!(formatted.contains("### Likely Tests to Run (25 returned)"));
        assert!(
            formatted.contains("  - `test_25` [line 250]"),
            "{formatted}"
        );
        assert!(!formatted.contains("hidden by the compact"));

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
    fn format_context_slice_does_not_repeat_expression_already_in_signature() {
        let body = "=>\n        transport.SendAsync<Response>(\n            HttpMethod.Get,\n            $\"items/{id}\",\n            cancellationToken: cancellationToken)";
        let mut slice = sample_context_slice();
        slice.target_symbol.language = "csharp".into();
        slice.target_symbol.signature = Some(format!(
            "public static Task<Response?> GetByIdAsync(\n        int id,\n        CancellationToken cancellationToken = default){body}"
        ));
        slice.target_body = body.into();

        let formatted = format_context_slice(&slice);

        assert_eq!(
            formatted.matches("transport.SendAsync").count(),
            1,
            "{formatted}"
        );
    }

    #[test]
    fn context_slice_leaves_out_a_signature_that_the_body_already_holds() {
        let mut slice = sample_context_slice();
        slice.target_symbol.signature = Some("type Person = { Name: string; Age: int }".into());
        slice.target_body = "type Person = { Name: string; Age: int }\n".into();

        let text = format_context_slice(&slice);

        assert!(!text.contains("Signature:"), "{text}");
        assert_eq!(text.matches("type Person").count(), 1, "{text}");
    }

    #[test]
    fn context_slice_says_when_no_test_was_found() {
        let text = format_context_slice(&sample_context_slice());

        assert!(
            text.ends_with("### Related Tests:\nNo test calls, uses, or names this symbol; blast_radius lists tests that can reach it through callers.\n"),
            "{text}"
        );
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
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            test_file_ceiling_reached: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("### Downstream Impact (200 returned)\n"));
    }

    #[test]
    fn blast_radius_does_not_claim_no_results_after_discovery_ceiling() {
        let res = BlastRadiusResult {
            seed_type: "file".into(),
            seeds: vec!["src/widget.rs".into()],
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            traversal_ceiling_reached: true,
            test_file_ceiling_reached: true,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("Likely Tests to Run (0 returned)"));
        assert!(formatted.contains("Downstream Impact (0 returned)"));
        assert!(formatted.contains("Name-matched test discovery hit its fixed ceiling"));
        assert!(!formatted.contains("No direct or name-matched tests found"));
        assert!(!formatted.contains("No downstream callers found within depth"));

        let no_ceiling = BlastRadiusResult {
            traversal_ceiling_reached: false,
            test_file_ceiling_reached: false,
            limit_at_maximum: false,
            ..res.clone()
        };
        assert!(format_blast_radius(&no_ceiling).contains("No direct or name-matched tests found"));

        let traversal_only = BlastRadiusResult {
            traversal_ceiling_reached: true,
            test_file_ceiling_reached: false,
            limit_at_maximum: false,
            ..res
        };
        let traversal_only_text = format_blast_radius(&traversal_only);
        assert!(!traversal_only_text.contains("No direct or name-matched tests found"));
    }

    fn skeleton_row(
        id: &str,
        parent: Option<&str>,
        kind: &str,
        name: &str,
        signature: &str,
        lines: (usize, usize),
        body: Option<(usize, usize)>,
    ) -> Symbol {
        Symbol {
            symbol_id: id.into(),
            file_id: "f1".into(),
            path: "src/lib.rs".into(),
            language: "rust".into(),
            name: name.into(),
            kind: kind.into(),
            signature: Some(signature.into()),
            doc_comment: None,
            visibility: None,
            parent_symbol_id: parent.map(str::to_string),
            start_line: lines.0,
            start_column: 0,
            end_line: lines.1,
            end_column: 1,
            start_byte: 0,
            end_byte: 0,
            body_start_line: body.map(|b| b.0),
            body_start_column: None,
            body_end_line: body.map(|b| b.1),
            body_end_column: None,
            body_start_byte: None,
            body_end_byte: None,
            body_hash: None,
            semantic_group: None,
            is_test: false,
            test_container: false,
        }
    }

    #[test]
    fn skeleton_nests_an_object_under_the_field_that_declares_it() {
        let syms = vec![
            skeleton_row(
                "root",
                None,
                "class",
                "shell",
                "extends ShellRoot",
                (1, 8),
                None,
            ),
            skeleton_row(
                "timer",
                Some("root"),
                "field",
                "localPluginReloadTimer",
                "localPluginReloadTimer: Timer",
                (2, 7),
                Some((2, 7)),
            ),
            skeleton_row(
                "interval",
                Some("timer"),
                "property",
                "interval",
                "interval: 150",
                (3, 3),
                None,
            ),
            skeleton_row(
                "fire",
                Some("timer"),
                "function",
                "fire",
                "function fire()",
                (5, 7),
                Some((6, 7)),
            ),
        ];

        assert_eq!(
            format_file_skeleton("shell/shell.qml", &syms, Some(8), 0),
            "// File: shell/shell.qml (Lines 1-8)\n\
             \n\
             extends ShellRoot {\n\
             \x20   localPluginReloadTimer: Timer {\n\
             \x20       interval: 150; // L3-3\n\
             \x20       function fire() { /* 2 lines hidden: L6-L7 */ }\n\
             \x20   } // L2-7\n\
             \n\
             } // L1-8\n\
             \n"
        );
    }

    #[test]
    fn skeleton_renders_a_single_line_symbol_with_children_as_a_leaf() {
        let syms = vec![
            skeleton_row(
                "rusqlite",
                None,
                "field",
                "rusqlite",
                "rusqlite = { workspace = true }",
                (15, 15),
                None,
            ),
            skeleton_row(
                "workspace",
                Some("rusqlite"),
                "property",
                "workspace",
                "workspace = true",
                (15, 15),
                None,
            ),
        ];

        assert_eq!(
            format_file_skeleton("Cargo.toml", &syms, Some(15), 0),
            "// File: Cargo.toml (Lines 1-15)\n\
             \n\
             rusqlite = { workspace = true }; // L15-15\n"
        );
    }

    #[test]
    fn skeleton_renders_members_of_a_single_line_enum() {
        let mut syms = vec![
            skeleton_row(
                "mode",
                None,
                "enum",
                "DispatchMode",
                "public enum DispatchMode",
                (21, 21),
                None,
            ),
            skeleton_row(
                "provision",
                Some("mode"),
                "enum_member",
                "Provision",
                "Provision",
                (21, 21),
                None,
            ),
            skeleton_row(
                "deprovision",
                Some("mode"),
                "enum_member",
                "Deprovision",
                "Deprovision",
                (21, 21),
                None,
            ),
            skeleton_row(
                "extension",
                Some("mode"),
                "enum_member",
                "Extension",
                "Extension",
                (21, 21),
                None,
            ),
        ];
        for symbol in &mut syms {
            symbol.language = "csharp".into();
        }

        let skeleton = format_file_skeleton("DispatchContext.cs", &syms, Some(21), 0);

        for member in ["Provision", "Deprovision", "Extension"] {
            assert!(skeleton.contains(member), "{skeleton}");
        }
    }

    #[test]
    fn a_hidden_body_names_the_functions_defined_inside_it_but_not_lambdas() {
        let mut syms = vec![
            skeleton_row(
                "factory",
                None,
                "function",
                "create_app",
                "def create_app()",
                (1, 9),
                Some((2, 9)),
            ),
            skeleton_row(
                "hello",
                Some("factory"),
                "function",
                "hello",
                "def hello()",
                (4, 5),
                Some((5, 5)),
            ),
            skeleton_row(
                "lambda",
                Some("factory"),
                "function",
                "lambda_7",
                "lambda v: v",
                (7, 7),
                None,
            ),
        ];
        for symbol in &mut syms {
            symbol.language = "python".into();
        }

        let skeleton = format_file_skeleton("app.py", &syms, Some(9), 0);

        assert!(
            skeleton.contains("def create_app() { /* 8 lines hidden: L2-L9; defines `hello` */ }"),
            "{skeleton}"
        );

        for symbol in &mut syms {
            symbol.language = "cpp".into();
        }
        let cpp = format_file_skeleton("app.cpp", &syms, Some(9), 0);
        assert!(!cpp.contains("defines"), "{cpp}");
        syms[1].test_container = true;
        let cpp_section = format_file_skeleton("app.cpp", &syms, Some(9), 0);
        assert!(cpp_section.contains("defines `hello`"), "{cpp_section}");
    }

    #[test]
    fn skeleton_keeps_plain_fields_and_function_locals_unchanged() {
        let syms = vec![
            skeleton_row(
                "cfg",
                None,
                "struct",
                "Config",
                "pub struct Config",
                (1, 3),
                None,
            ),
            skeleton_row(
                "retries",
                Some("cfg"),
                "field",
                "retries",
                "pub retries: u32",
                (2, 2),
                None,
            ),
            skeleton_row(
                "run",
                None,
                "function",
                "run",
                "pub fn run()",
                (5, 9),
                Some((6, 8)),
            ),
            skeleton_row(
                "tmp",
                Some("run"),
                "variable",
                "tmp",
                "let tmp",
                (7, 7),
                None,
            ),
        ];

        assert_eq!(
            format_file_skeleton("src/lib.rs", &syms, Some(9), 0),
            "// File: src/lib.rs (Lines 1-9)\n\
             \n\
             pub struct Config {\n\
             \x20   pub retries: u32; // L2-2\n\
             } // L1-3\n\
             \n\
             pub fn run() { /* 3 lines hidden: L6-L8 */ }\n"
        );
    }

    #[test]
    fn skeleton_marks_an_event_row_whose_signature_does_not_spell_it() {
        let syms = vec![
            skeleton_row(
                "cls",
                None,
                "class",
                "ColumnViewAttached",
                "class ColumnViewAttached : public QObject",
                (11, 52),
                None,
            ),
            skeleton_row(
                "sig",
                Some("cls"),
                "event",
                "indexChanged",
                "void indexChanged()",
                (47, 47),
                None,
            ),
            skeleton_row(
                "qml",
                None,
                "event",
                "clicked",
                "signal clicked()",
                (60, 60),
                None,
            ),
        ];

        assert_eq!(
            format_file_skeleton("src/columnview.h", &syms, Some(60), 0),
            "// File: src/columnview.h (Lines 1-60)\n\
             \n\
             class ColumnViewAttached : public QObject {\n\
             \x20   void indexChanged(); // event L47-47\n\
             } // L11-52\n\
             \n\
             signal clicked(); // L60-60\n"
        );
    }
}
