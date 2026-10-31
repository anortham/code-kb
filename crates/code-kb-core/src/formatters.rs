use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use crate::edit_check::{BaseSource, EditCheck, EditSite, ShapeChange};
use crate::models::{
    BlastRadiusResult, CallersReportSites, CallingTest, ConstructionSite, ContextSlice,
    ImpactedSymbol, LabeledSite, OpenEnd, OverrideRow, ReferenceReport, ReferenceSite,
    Registration, SearchExplain, SiteLabel, Symbol, SymbolSearchResult, VariantUse,
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
        out.push_str("// No symbols are indexed for this file.\n");
        return out;
    }

    let children_map = children_in_file(symbols);
    let roots: Vec<&Symbol> = match children_map.get(&None) {
        Some(roots) => roots.clone(),
        None => symbols.iter().collect(),
    };
    if !roots.iter().any(|root| is_shown(root, None)) {
        out.push_str(&hidden_rows_summary(&roots));
        return out;
    }
    let budget = OUTPUT_BUDGET_CHARS.saturating_sub(out.len() + SKELETON_CUT_NOTE.len());
    let (rows, cut) = render_rows_within(&roots, None, &children_map, 0, budget);
    if cut {
        out.push_str(SKELETON_CUT_NOTE);
    }
    out.push_str(&rows);
    out
}

/// The most characters that a skeleton, a body, or a context answer prints: about 8K tokens,
/// below the 10K tokens at which Claude Code warns about a large MCP answer.
pub const OUTPUT_BUDGET_CHARS: usize = 32_000;

const SKELETON_CUT_NOTE: &str = "// Cut to about 8K tokens: each `rows` comment gives the lines it hides. Read those lines, or call lookup_symbol with `Name.` to list the members of a type.\n\n";

type ChildrenMap<'a> = HashMap<Option<String>, Vec<&'a Symbol>>;

/// The rows of one file under their parents. A Rust `impl` block in this file can belong to a
/// type in another file; its methods are roots here.
fn children_in_file(symbols: &[Symbol]) -> ChildrenMap<'_> {
    let in_file: HashSet<&str> = symbols.iter().map(|s| s.symbol_id.as_str()).collect();
    let mut children_map: ChildrenMap = HashMap::new();
    for s in symbols {
        let parent = s
            .parent_symbol_id
            .clone()
            .filter(|parent| in_file.contains(parent.as_str()));
        children_map.entry(parent).or_default().push(s);
    }
    children_map
}

/// The rows a cut skeleton prints: the first `admitted` shown rows in breadth-first order, so
/// the outer structure stays and the deepest rows go first. A row with no rank prints.
#[derive(Clone, Copy)]
struct RowCut<'a> {
    rank: &'a HashMap<&'a str, usize>,
    admitted: usize,
}

fn admits(cut: Option<RowCut>, sym: &Symbol) -> bool {
    cut.is_none_or(|cut| {
        cut.rank
            .get(sym.symbol_id.as_str())
            .is_none_or(|rank| *rank < cut.admitted)
    })
}

/// The skeleton rows of `roots` within `budget` characters, and whether a cut left rows out.
fn render_rows_within(
    roots: &[&Symbol],
    parent: Option<&Symbol>,
    children_map: &ChildrenMap,
    indent_level: usize,
    budget: usize,
) -> (String, bool) {
    let render = |cut: Option<RowCut>| {
        let mut out = String::new();
        for root in roots {
            render_symbol_skeleton(&mut out, root, parent, children_map, indent_level, cut);
        }
        push_fold(&mut out, roots, parent, children_map, cut, indent_level);
        out
    };
    let full = render(None);
    if full.len() <= budget {
        return (full, false);
    }
    let order = breadth_first_rows(roots, parent, children_map);
    let rank: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(index, sym)| (sym.symbol_id.as_str(), index))
        .collect();
    let render_first = |admitted| {
        render(Some(RowCut {
            rank: &rank,
            admitted,
        }))
    };
    let (mut low, mut high) = (0, order.len());
    while low < high {
        let mid = (low + high).div_ceil(2);
        if render_first(mid).len() <= budget {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    // Admitting the last hidden row of a group drops its fold comment, so a longer prefix can
    // render shorter than one the search rejected.
    let best = (low + 1..=(low + FOLD_LOOKAHEAD_ROWS).min(order.len()))
        .rev()
        .find(|admitted| render_first(*admitted).len() <= budget)
        .unwrap_or(low);
    (render_first(best), true)
}

const FOLD_LOOKAHEAD_ROWS: usize = 16;

fn breadth_first_rows<'a>(
    roots: &[&'a Symbol],
    parent: Option<&'a Symbol>,
    children_map: &ChildrenMap<'a>,
) -> Vec<&'a Symbol> {
    let mut order = Vec::new();
    let mut queue: std::collections::VecDeque<(&Symbol, Option<&Symbol>)> =
        roots.iter().map(|root| (*root, parent)).collect();
    while let Some((sym, parent)) = queue.pop_front() {
        if !is_skeleton_row_shown(sym, parent) {
            continue;
        }
        order.push(sym);
        if let Some(children) = children_map.get(&Some(sym.symbol_id.clone())) {
            queue.extend(children.iter().map(|child| (*child, Some(sym))));
        }
    }
    order
}

/// The first and last line of `sym` and of every row below it, which can lie outside its own
/// span (a Rust method in another `impl` block of the file).
fn subtree_lines(sym: &Symbol, children_map: &ChildrenMap) -> (usize, usize) {
    let mut lines = (sym.start_line, sym.end_line);
    let mut stack = vec![sym];
    while let Some(row) = stack.pop() {
        for child in children_map
            .get(&Some(row.symbol_id.clone()))
            .into_iter()
            .flatten()
        {
            lines = (lines.0.min(child.start_line), lines.1.max(child.end_line));
            stack.push(child);
        }
    }
    lines
}

/// The lines of `rows` and of every row below them.
fn rows_lines(rows: &[&Symbol], children_map: &ChildrenMap) -> Option<(usize, usize)> {
    rows.iter()
        .map(|row| subtree_lines(row, children_map))
        .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
}

/// One comment for the shown `rows` that a cut leaves out, with the lines they and the rows
/// below them span.
fn push_fold(
    out: &mut String,
    rows: &[&Symbol],
    parent: Option<&Symbol>,
    children_map: &ChildrenMap,
    cut: Option<RowCut>,
    indent_level: usize,
) {
    let hidden: Vec<&Symbol> = rows
        .iter()
        .copied()
        .filter(|row| is_skeleton_row_shown(row, parent) && !admits(cut, row))
        .collect();
    let Some((first, last)) = rows_lines(&hidden, children_map) else {
        return;
    };
    let indent = "    ".repeat(indent_level);
    let noun = if hidden.len() == 1 { "row" } else { "rows" };
    out.push_str(&format!(
        "{indent}/* {} more {noun}: L{first}-L{last} */\n",
        hidden.len()
    ));
}

/// The skeleton of `target` and of the rows below it in `file_symbols` within `budget`
/// characters, or `None` when no row below it shows.
pub fn format_member_skeleton(
    target: &Symbol,
    file_symbols: &[Symbol],
    budget: usize,
) -> Option<String> {
    let children_map = children_in_file(file_symbols);
    let has_shown_member = children_map
        .get(&Some(target.symbol_id.clone()))
        .is_some_and(|members| {
            members
                .iter()
                .any(|member| is_skeleton_row_shown(member, Some(target)))
        });
    if !has_shown_member {
        return None;
    }
    let parent = target.parent_symbol_id.as_deref().and_then(|parent| {
        file_symbols
            .iter()
            .find(|symbol| symbol.symbol_id == parent)
    });
    Some(render_rows_within(&[target], parent, &children_map, 0, budget).0)
}

/// The answer for a file whose symbols are all hidden from the skeleton: the top-level rows
/// grouped by kind, with the first three names of each group.
/// `name` cut to 120 characters with `…`, so a generated name cannot flood a one-line summary.
fn cut_name(name: &str) -> std::borrow::Cow<'_, str> {
    const MAX_CHARS: usize = 120;
    match name.char_indices().nth(MAX_CHARS) {
        Some((end, _)) => format!("{}…", &name[..end]).into(),
        None => name.into(),
    }
}

fn hidden_rows_summary(roots: &[&Symbol]) -> String {
    const NAMES_PER_KIND: usize = 3;
    let mut groups: Vec<(&str, Vec<&Symbol>)> = Vec::new();
    for root in roots {
        let kind = display_kind(root);
        match groups
            .iter_mut()
            .find(|(group_kind, _)| *group_kind == kind)
        {
            Some((_, rows)) => rows.push(root),
            None => groups.push((kind, vec![root])),
        }
    }
    let groups: Vec<String> = groups
        .iter()
        .map(|(kind, rows)| {
            let mut names: Vec<String> = rows
                .iter()
                .take(NAMES_PER_KIND)
                .map(|row| format!("`{}` L{}", cut_name(&row.name), row.start_line))
                .collect();
            if rows.len() > NAMES_PER_KIND {
                names.push(format!("+{} more", rows.len() - NAMES_PER_KIND));
            }
            format!(
                "{} {} ({})",
                rows.len(),
                plural(rows.len(), kind),
                names.join(", ")
            )
        })
        .collect();
    format!(
        "// Nothing to outline. Top-level rows: {}.\n\
         // get_symbol_body with one of these names and this file_path shows its source; read the file for the rest.\n",
        groups.join("; ")
    )
}

/// The skeleton of a file whose symbols the index cannot hold: `reason` says why.
pub fn format_unindexed_file(file_path: &str, reason: &str) -> String {
    format!("// File: {file_path}\n\n// {reason} Read the file.\n")
}

pub(crate) fn is_container_kind(kind: &str) -> bool {
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
pub(crate) fn container_signature<'a>(sig: &'a str, sym: &'a Symbol) -> std::borrow::Cow<'a, str> {
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
    let css_custom_property =
        sym.language == "css" && sym.kind == "property" && sym.name.starts_with("--");
    let nested_css_declaration = sym.language == "css"
        && sym.kind == "property"
        && parent.is_some_and(|parent| parent.language == "css" && parent.kind == "property");
    if nested_css_declaration {
        return css_custom_property;
    }
    !is_skippable_kind(&sym.kind) || python_declaration || css_custom_property
}

fn is_structural_inline_child(sym: &Symbol, parent: Option<&Symbol>) -> bool {
    let Some(parent) = parent else {
        return false;
    };
    if !matches!(
        sym.language.as_str(),
        "html" | "ini" | "json" | "toml" | "vue" | "xml" | "yaml"
    ) || sym.language != parent.language
        || sym.start_line != parent.start_line
        || parent.start_line != parent.end_line
    {
        return false;
    }
    let owner_signature = parent.signature.as_deref().unwrap_or(&parent.name);
    let child_signature = sym.signature.as_deref().unwrap_or(&sym.name);
    !child_signature.is_empty() && !owner_signature.contains(child_signature)
}

fn is_skeleton_row_shown(sym: &Symbol, parent: Option<&Symbol>) -> bool {
    is_shown(sym, parent) || is_structural_inline_child(sym, parent)
}

/// The rows of a public skeleton and how many rows of the full skeleton it hides: a row whose
/// visibility is in [`crate::queries::HIDDEN_VISIBILITIES`] goes, with every row below it.
pub fn public_surface(symbols: &[Symbol]) -> (Vec<Symbol>, usize) {
    let by_id: HashMap<&str, &Symbol> = symbols.iter().map(|s| (s.symbol_id.as_str(), s)).collect();
    let parent_of = |sym: &Symbol| {
        sym.parent_symbol_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied())
    };
    let hidden = |sym: &Symbol| {
        let mut row = Some(sym);
        let mut steps = 0;
        while let Some(current) = row {
            if crate::queries::is_hidden_visibility(current.visibility.as_deref()) {
                return true;
            }
            steps += 1;
            if steps > symbols.len() {
                return false;
            }
            row = parent_of(current);
        }
        false
    };
    let mut kept = Vec::new();
    let mut hidden_rows = 0;
    for sym in symbols {
        if !hidden(sym) {
            kept.push(sym.clone());
        } else if is_skeleton_row_shown(sym, parent_of(sym)) {
            hidden_rows += 1;
        }
    }
    (kept, hidden_rows)
}

/// The line under a public skeleton's file header, or the whole body when no row is public.
pub fn public_surface_note(hidden_rows: usize, any_public: bool) -> String {
    let rows = format!("{hidden_rows} private {}", plural(hidden_rows, "row"));
    if any_public {
        format!("// Public surface: {rows} hidden.\n\n")
    } else {
        format!("// No public rows: {rows} hidden.\n")
    }
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
    children_map: &ChildrenMap,
    indent_level: usize,
    cut: Option<RowCut>,
) {
    if !is_skeleton_row_shown(sym, parent) || !admits(cut, sym) {
        return;
    }

    let indent = "    ".repeat(indent_level);

    if let Some(ref doc) = sym.doc_comment {
        let document = crate::queries::DOCUMENTATION_LANGUAGES.contains(&sym.language.as_str());
        let lines: Vec<_> = if document {
            doc.lines().collect()
        } else {
            doc.lines()
                .map(doc_line_text)
                .filter(|line| !line.is_empty())
                .collect()
        };
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
    let css_rule_has_custom_property = sym.language == "css"
        && sym.kind == "property"
        && children.is_some_and(|list| {
            list.iter().any(|child| {
                child.language == "css" && child.kind == "property" && child.name.starts_with("--")
            })
        });

    if (is_container_kind(&sym.kind) || css_rule_has_custom_property)
        && children.is_some_and(|list| {
            list.iter()
                .any(|child| is_skeleton_row_shown(child, Some(sym)))
        })
        && (spans_multiple_lines || sym.kind == "enum" || css_rule_has_custom_property)
    {
        let raw_sig = sym.signature.as_deref().unwrap_or(&sym.name);
        let sig = container_signature(raw_sig, sym);
        let child_list = children.map(Vec::as_slice).unwrap_or_default();
        let shown: Vec<&Symbol> = child_list
            .iter()
            .copied()
            .filter(|child| is_skeleton_row_shown(child, Some(sym)))
            .collect();
        if !shown.iter().any(|child| admits(cut, child)) {
            let noun = if shown.len() == 1 { "row" } else { "rows" };
            let (first, last) = subtree_lines(sym, children_map);
            out.push_str(&format!(
                "{indent}{sig} {{ /* {} {noun} hidden: L{first}-L{last} */ }}\n",
                shown.len()
            ));
            return;
        }
        out.push_str(&format!("{indent}{sig} {{\n"));
        for child in child_list {
            render_symbol_skeleton(out, child, Some(sym), children_map, indent_level + 1, cut);
        }
        push_fold(
            out,
            child_list,
            Some(sym),
            children_map,
            cut,
            indent_level + 1,
        );
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
                render_symbol_skeleton(out, child, Some(sym), children_map, indent_level, cut);
            }
            push_fold(out, child_list, Some(sym), children_map, cut, indent_level);
        } else if let Some(child_list) = children
            && child_list.iter().any(|child| {
                child.start_line > sym.end_line || is_structural_inline_child(child, Some(sym))
            })
        {
            let after: Vec<&Symbol> = child_list
                .iter()
                .copied()
                .filter(|child| child.start_line > sym.end_line)
                .collect();
            for child in &after {
                render_symbol_skeleton(out, child, Some(sym), children_map, indent_level, cut);
            }
            push_fold(out, &after, Some(sym), children_map, cut, indent_level);
            let inline: Vec<&Symbol> = child_list
                .iter()
                .copied()
                .filter(|child| is_structural_inline_child(child, Some(sym)))
                .collect();
            for child in &inline {
                render_symbol_skeleton(out, child, Some(sym), children_map, indent_level + 1, cut);
            }
            push_fold(out, &inline, Some(sym), children_map, cut, indent_level + 1);
        }
    }
}

/// A file with definitions or tests in the outline: its first definitions as `kind name`, and
/// its counts.
#[derive(Default)]
pub struct OutlineFile {
    pub names: Vec<String>,
    pub definitions: usize,
    pub tests: usize,
    pub fixtures: usize,
    /// Top-level definitions that a public outline hides.
    pub private: usize,
}

impl OutlineFile {
    /// The bracketed list after the file name: up to `names_listed` names, then the counts.
    fn tags(&self, names_listed: usize) -> String {
        let mut tags: Vec<String> = self.names.iter().take(names_listed).cloned().collect();
        match (tags.len(), self.definitions) {
            (_, 0) => {}
            (0, count) => tags.push(format!("{count} {}", plural(count, "definition"))),
            (shown, count) if count > shown => tags.push(format!("+{} more", count - shown)),
            _ => {}
        }
        if self.private > 0 {
            tags.push(format!("{} private", self.private));
        }
        if self.tests > 0 {
            tags.push(format!("{} {}", self.tests, plural(self.tests, "test")));
        }
        if self.fixtures > 0 {
            tags.push(format!(
                "{} {}",
                self.fixtures,
                plural(self.fixtures, "fixture")
            ));
        }
        format!(" [{}]", tags.join(", "))
    }
}

/// Node representing directory or file in codebase outline tree.
#[derive(Default)]
pub struct OutlineNode {
    pub files: BTreeMap<String, OutlineFile>,
    pub subdirs: BTreeMap<String, OutlineNode>,
    /// Files under this directory that lie below the depth limit.
    pub hidden_files: usize,
    /// Indexed files in this directory with no function, class, or test to list.
    pub plain_files: Vec<String>,
    /// Files under this directory that no extractor reads.
    pub unsupported_files: usize,
}

impl OutlineNode {
    /// The indexed and the unsupported files in this folder and every folder below it.
    fn files_below(&self) -> (usize, usize) {
        self.subdirs.values().map(OutlineNode::files_below).fold(
            (
                self.files.len() + self.plain_files.len() + self.hidden_files,
                self.unsupported_files,
            ),
            |(indexed, unsupported), (sub_indexed, sub_unsupported)| {
                (indexed + sub_indexed, unsupported + sub_unsupported)
            },
        )
    }
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
                let names: Vec<String> = symbols_by_file
                    .get(&normalized)
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_test && !s.test_container)
                    .take(OUTLINE_NAMES_PER_FILE)
                    .map(|s| format!("{} {}", display_kind(s), s.name))
                    .collect();
                let count = counts.get(&normalized).copied().unwrap_or_default();
                let file = OutlineFile {
                    definitions: count.definitions.max(names.len()),
                    names,
                    tests: count.tests,
                    fixtures: count.fixtures,
                    private: count.private,
                };
                if file.definitions + file.tests + file.fixtures + file.private == 0 {
                    curr.plain_files.push(comp.to_string());
                } else {
                    curr.files.insert(comp.to_string(), file);
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

/// The most definitions an outline names for one file before it counts the rest.
pub const OUTLINE_NAMES_PER_FILE: usize = 5;

/// The most files with definitions a subfolder lists; the folder the outline starts at lists all.
const FILES_LISTED_PER_FOLDER: usize = 40;

pub fn render_outline_tree(
    out: &mut String,
    node: &OutlineNode,
    prefix: &str,
    depth: usize,
    max_depth: usize,
    names_per_file: usize,
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

        let (hidden_files, unsupported_files) = if depth + 1 >= max_depth {
            sub.files_below()
        } else {
            (sub.hidden_files, sub.unsupported_files)
        };
        let hidden = match (hidden_files, unsupported_files) {
            (0, 0) => String::new(),
            (0, m) => format!(" ({m} unsupported {})", plural(m, "file")),
            (n, 0) => format!(" ({n} indexed {})", plural(n, "file")),
            (n, m) => format!(" ({n} indexed {}, {m} unsupported)", plural(n, "file")),
        };
        out.push_str(&format!("{prefix}{branch}{name}/{hidden}\n"));
        render_outline_tree(out, sub, &next_prefix, depth + 1, max_depth, names_per_file);
    }

    // Render files
    for (file_name, file) in node.files.iter().take(listed) {
        index += 1;
        let is_last = index == total_items;
        let branch = if is_last { "└── " } else { "├── " };
        let tags = file.tags(names_per_file);
        out.push_str(&format!("{prefix}{branch}{file_name}{tags}\n"));
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

/// The bodies of one symbol or of its overloads, each under its own location comment.
pub fn format_symbol_bodies(bodies: &[(Symbol, String)]) -> String {
    let mut out = overloads_header(bodies.iter().map(|(symbol, _)| symbol));
    let parts: Vec<String> = bodies
        .iter()
        .map(|(symbol, source)| format_symbol_body(symbol, source))
        .collect();
    out.push_str(&parts.join("\n"));
    out
}

/// The context of one symbol or of each of its overloads.
pub fn format_context_slices(slices: &[ContextSlice]) -> String {
    let mut out = overloads_header(slices.iter().map(|slice| &slice.target_symbol));
    let parts: Vec<String> = slices.iter().map(format_context_slice).collect();
    out.push_str(&parts.join("\n"));
    out
}

fn overloads_header<'a>(symbols: impl ExactSizeIterator<Item = &'a Symbol>) -> String {
    let count = symbols.len();
    let mut symbols = symbols;
    match symbols.next() {
        Some(first) if count > 1 => format!(
            "`{}` has {count} overloads in {}; each one follows.\n\n",
            first.name, first.path
        ),
        _ => String::new(),
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
            "### Related Tests:\nNo related test was found in the index; blast_radius may find tests through callers.\n",
        );
    }

    out
}

/// The answer of every paged or limited report, CLI and MCP alike, when its limit is 0.
pub const ZERO_LIMIT_NOTICE: &str = "Result limit is 0; increase it to check for matches.\n";

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
            caller_text(r)
        } else {
            format!("`{}`", callee_name(&r.to_symbol_name))
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
            "- {other} [{}{line_info}] (kind: {}{in_file}){target}\n",
            r.path, r.kind
        ));
    }

    if refs.len() >= limit {
        out.push_str(&cap_notice(refs.len(), limit));
    }

    out
}

/// Format the requested window from a bounded references query result.
pub fn format_references_page(
    target_name: &str,
    refs: &[ReferenceSite],
    direction: &str,
    limit: usize,
    offset: usize,
) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    if limit == 0 {
        return ZERO_LIMIT_NOTICE.to_string();
    }
    if offset == 0 {
        return format_references(target_name, refs, direction, limit);
    }
    let (start, end) = page_bounds(refs.len(), offset, limit);
    if start == end {
        return format!(
            "Page offset {offset}: no rows within the {} queried results.\n",
            refs.len()
        );
    }
    format!(
        "Page offset {offset}: showing {} rows from {} queried results.\n\n{}",
        end - start,
        refs.len(),
        format_references(target_name, &refs[start..end], direction, limit)
    )
}

/// Format a paged callee query whose rows and complete count came from SQL LIMIT/OFFSET.
pub fn format_references_window(
    target_name: &str,
    refs: &[ReferenceSite],
    direction: &str,
    limit: usize,
    offset: usize,
    total: usize,
) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    if limit == 0 {
        return ZERO_LIMIT_NOTICE.to_string();
    }
    if offset == 0 && total <= limit {
        return format_references(target_name, refs, direction, limit.saturating_add(1));
    }
    if refs.is_empty() {
        if offset == 0 {
            return format_references(target_name, refs, direction, limit.saturating_add(1));
        }
        return format!("Page offset {offset}: no rows within the {total} queried results.\n");
    }

    let dir_label = if direction == "callers" {
        "Callers of"
    } else {
        "Callees called by"
    };
    let mut out = if offset == 0 {
        format!("{dir_label} `{target_name}` ({total} found):\n")
    } else {
        format!(
            "Page offset {offset}: showing {} {} from {total} queried results.\n\n{dir_label} `{target_name}` ({total} found):\n",
            refs.len(),
            plural(refs.len(), "row")
        )
    };
    let page_body = format_references(target_name, refs, direction, limit.saturating_add(1));
    if let Some((_, body)) = page_body.split_once('\n') {
        out.push_str(body);
    }
    if offset == 0 && refs.len() < total {
        out.push_str(&format!(
            "\n[Showing {} of {total} results; pass offset {} to see more.]\n",
            refs.len(),
            refs.len()
        ));
    }
    out
}

fn caller_text(site: &ReferenceSite) -> String {
    if site.from_symbol_name.is_empty() && site.enclosing_symbol_name.is_none() {
        "(no enclosing symbol)".to_string()
    } else {
        format!(
            "`{}`",
            displayed_symbol_name(
                &site.from_symbol_name,
                site.enclosing_symbol_name.as_deref()
            )
        )
    }
}

const FOLD_FILES_NAMED: usize = 5;

/// One line for languages whose julie capability table records no reference sites, so an answer
/// does not read as a complete list of uses in their files.
pub fn coverage_gap_line(gap: Option<&crate::models::CoverageGap>) -> String {
    let Some(gap) = gap.filter(|gap| !gap.languages.is_empty()) else {
        return String::new();
    };
    format!(
        "Coverage: the index records no reference sites in {} files (julie: {} exception), so this answer lists no site inside them.\n",
        series(&gap.languages),
        series(&gap.capabilities)
    )
}

/// `a`, `a and b`, or `a, b, and c`.
fn series(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first, second] => format!("{first} and {second}"),
        [head @ .., last] => format!("{}, and {last}", head.join(", ")),
    }
}

/// The callers answer: three header lines, up to `limit` sites of each label, the reach sections,
/// up to `limit` text-only lines, the blind spots of the index, and the import summary. Each
/// section lists the rows of the first page window, so `offset = limit` continues every section.
/// Counts are always complete.
pub fn format_reference_report(report: &ReferenceReport, limit: usize) -> String {
    let target = &report.target;
    let counts = &report.counts;
    let mut out = format!(
        "References to {} `{}` ({}:{})\nSites: {} · proven {} · likely {} · unresolved {} · excluded {}\n",
        target.kind,
        target.display_name,
        target.path,
        target.line,
        counts.sites,
        counts.proven,
        counts.likely,
        counts.unresolved,
        counts.excluded,
    );
    out.push_str(&text_count_line(report));
    let scan = &report.text_scan;
    if !scan.complete && scan.skipped.is_none() {
        out.push_str(&format!(
            "Text scan stopped after {} s: {} of {} files read; counts cover those files.\n",
            (scan.elapsed_ms + 500) / 1000,
            scan.files_scanned,
            scan.files_total
        ));
    }
    out.push_str(&coverage_gap_line(report.coverage_gap.as_ref()));

    let mut body = site_groups(&report.sites, limit);
    body.push_str(&override_sections(report, limit));
    body.push_str(&construction_section(&report.constructions, limit));
    body.push_str(&variant_section(&report.variant_uses, limit));
    body.push_str(&registration_section(&report.registrations, limit));
    body.push_str(&test_section(&report.tests, limit));
    body.push_str(&text_only_section(report, limit));
    if !report.cannot_see.is_empty() {
        body.push_str(&format!(
            "Not visible to this index: {}.\n",
            report.cannot_see.join("; ")
        ));
    }
    body.push_str(&format_import_summary(&report.imports));
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

/// Format a caller report page from the current report without changing its complete counts.
pub fn format_reference_report_page(
    report: &ReferenceReport,
    limit: usize,
    offset: usize,
) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    if limit == 0 {
        return ZERO_LIMIT_NOTICE.to_string();
    }
    if offset == 0 {
        return format_reference_report(report, limit);
    }

    let target = &report.target;
    let counts = &report.counts;
    let mut out = format!(
        "References to {} `{}` ({}:{})\nSites: {} · proven {} · likely {} · unresolved {} · excluded {}\n",
        target.kind,
        target.display_name,
        target.path,
        target.line,
        counts.sites,
        counts.proven,
        counts.likely,
        counts.unresolved,
        counts.excluded,
    );
    out.push_str(&text_count_line(report));
    let scan = &report.text_scan;
    if !scan.complete && scan.skipped.is_none() {
        out.push_str(&format!(
            "Text scan stopped after {} s: {} of {} files read; counts cover those files.\n",
            (scan.elapsed_ms + 500) / 1000,
            scan.files_scanned,
            scan.files_total
        ));
    }
    out.push_str(&coverage_gap_line(report.coverage_gap.as_ref()));

    let mut body = site_groups_page(&report.sites, limit, offset);
    body.push_str(&override_sections_page(report, limit, offset));
    body.push_str(&construction_section_page(
        &report.constructions,
        limit,
        offset,
    ));
    body.push_str(&variant_section_page(&report.variant_uses, limit, offset));
    body.push_str(&registration_section_page(
        &report.registrations,
        limit,
        offset,
    ));
    body.push_str(&test_section_page(&report.tests, limit, offset));
    body.push_str(&text_only_section_page(report, limit, offset));
    if !report.cannot_see.is_empty() {
        body.push_str(&format!(
            "Not visible to this index: {}.\n",
            report.cannot_see.join("; ")
        ));
    }
    body.push_str(&format_import_summary(&report.imports));
    if !body.is_empty() {
        out.push('\n');
        out.push_str(&body);
    }
    out
}

fn page_bounds(len: usize, offset: usize, limit: usize) -> (usize, usize) {
    let start = offset.min(len);
    (start, start + limit.min(len - start))
}

fn invalid_page_window(offset: usize, limit: usize) -> Option<String> {
    crate::queries::validate_result_page(offset, limit)
        .err()
        .map(|error| format!("Invalid report page: {error}\n"))
}

fn page_caption(out: &mut String, total: usize, start: usize, end: usize, offset: usize) {
    if start == end {
        out.push_str(&format!(
            "  Page offset {offset}: no rows; {total} total.\n"
        ));
    } else {
        out.push_str(&format!(
            "  Page offset {start}: rows {}-{} of {total}.\n",
            start + 1,
            end
        ));
    }
}

fn site_groups_page(sites: &[LabeledSite], limit: usize, offset: usize) -> String {
    let mut out = String::new();
    for label in SiteLabel::ALL {
        let group: Vec<&LabeledSite> = sites.iter().filter(|site| site.label == label).collect();
        if group.is_empty() {
            continue;
        }
        let title = match label {
            SiteLabel::Proven => "Proven",
            SiteLabel::Likely => "Likely",
            SiteLabel::Unresolved => "Unresolved",
            SiteLabel::Excluded => "Excluded",
        };
        let (start, end) = page_bounds(group.len(), offset, limit);
        out.push_str(&format!("{title} ({}):\n", group.len()));
        page_caption(&mut out, group.len(), start, end, offset);
        for labeled in &group[start..end] {
            let site = &labeled.site;
            let line = site
                .start_line
                .map(|line| format!(":{line}"))
                .unwrap_or_default();
            out.push_str(&format!(
                "- {} [{}{line}] (kind: {}; {}{})\n",
                caller_text(site),
                site.path,
                site.kind,
                labeled.basis,
                arg_count_text(labeled)
            ));
        }
        if end < group.len() {
            out.push_str(&fold_line(label, &group[end..]));
        }
    }
    out
}

fn override_sections_page(report: &ReferenceReport, limit: usize, offset: usize) -> String {
    let name = &report.target.display_name;
    let mut out = method_section_page(
        &format!(
            "Overrides ({}): calls through `{name}` can reach these",
            report.overrides.len()
        ),
        &report.overrides,
        limit,
        offset,
    );
    out.push_str(&method_section_page(
        &format!(
            "Overrides of ({}): calls to these can reach `{name}`",
            report.overridden.len()
        ),
        &report.overridden,
        limit,
        offset,
    ));
    out
}

fn method_section_page(title: &str, rows: &[OverrideRow], limit: usize, offset: usize) -> String {
    section_page(
        rows,
        limit,
        offset,
        format!("{title}\n"),
        "method",
        |row| row.path.as_str(),
        |row| {
            format!(
                "- `{}.{}` ({}:{})\n",
                row.owner, row.name, row.path, row.line
            )
        },
    )
}

fn section_page<'a, T>(
    rows: &'a [T],
    limit: usize,
    offset: usize,
    title: String,
    noun: &str,
    path: impl Fn(&'a T) -> &'a str,
    row_text: impl Fn(&T) -> String,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let (start, end) = page_bounds(rows.len(), offset, limit);
    let mut out = title;
    page_caption(&mut out, rows.len(), start, end, offset);
    for row in &rows[start..end] {
        out.push_str(&row_text(row));
    }
    if end < rows.len() {
        out.push_str(&section_fold(rows[end..].iter().map(path), noun));
    }
    out
}

fn construction_section_page(rows: &[ConstructionSite], limit: usize, offset: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let (start, end) = page_bounds(rows.len(), offset, limit);
    let mut per_file: Vec<(&str, Vec<String>)> = Vec::new();
    for row in &rows[start..end] {
        match per_file.last_mut() {
            Some((path, lines)) if *path == row.path => lines.push(row.line.to_string()),
            _ => per_file.push((&row.path, vec![row.line.to_string()])),
        }
    }
    let listed: Vec<String> = per_file
        .iter()
        .map(|(path, lines)| format!("{path}: {}", lines.join(", ")))
        .collect();
    let mut out = format!("Constructions ({}):", rows.len());
    if !listed.is_empty() {
        out.push_str(&format!(" {}", listed.join(" · ")));
    }
    out.push('\n');
    page_caption(&mut out, rows.len(), start, end, offset);
    if end < rows.len() {
        out.push_str(&section_fold(
            rows[end..].iter().map(|row| row.path.as_str()),
            "construction",
        ));
    }
    out
}

fn variant_section_page(rows: &[VariantUse], limit: usize, offset: usize) -> String {
    section_page(
        rows,
        limit,
        offset,
        format!(
            "Variant uses ({} {}): a match in them may need a new arm\n",
            rows.len(),
            plural(rows.len(), "function")
        ),
        "function",
        |row| row.path.as_str(),
        |row| {
            let caller = if row.caller.is_empty() {
                "(no enclosing symbol)".to_string()
            } else {
                format!("`{}`", row.caller)
            };
            format!(
                "- {caller} [{}:{}] names {} of {} {}\n",
                row.path,
                row.line,
                row.variants_used,
                row.variants_total,
                plural(row.variants_total, "variant"),
            )
        },
    )
}

fn registration_section_page(rows: &[Registration], limit: usize, offset: usize) -> String {
    section_page(
        rows,
        limit,
        offset,
        format!("Registrations ({}):\n", rows.len()),
        "registration",
        |row| row.path.as_str(),
        |row| {
            format!(
                "- {} [{}:{}]{}\n",
                registration_label(row),
                row.path,
                row.line,
                if row.by_name { " (by name)" } else { "" }
            )
        },
    )
}

fn test_section_page(rows: &[CallingTest], limit: usize, offset: usize) -> String {
    section_page(
        rows,
        limit,
        offset,
        format!("Tests that call or use it ({}):\n", rows.len()),
        "test",
        |row| row.path.as_str(),
        |row| {
            let run = row
                .command
                .as_ref()
                .map(|command| format!(" run: {command}"))
                .unwrap_or_default();
            let name_only = if row.name_only { " (name only)" } else { "" };
            format!(
                "- `{}` [{}:{}]{name_only}{run}\n",
                on_one_line(&row.name),
                row.path,
                row.line
            )
        },
    )
}

fn text_only_section_page(report: &ReferenceReport, limit: usize, offset: usize) -> String {
    let total = report
        .text_only
        .iter()
        .map(|file| file.lines.len())
        .sum::<usize>();
    if total == 0 {
        return String::new();
    }
    let (start, end) = page_bounds(total, offset, limit);
    let mut out = format!(
        "Text only ({}): docs, comments, strings, or code the index did not record\n",
        report.counts.text_only_lines
    );
    page_caption(&mut out, total, start, end, offset);
    out.push_str(&text_only_window(report, start, limit));
    out
}

fn text_count_line(report: &ReferenceReport) -> String {
    if let Some(class) = &report.text_scan.skipped {
        return format!(
            "Text: skipped for a constructor; its call sites spell the class name. Use find_references on `{class}`.\n"
        );
    }
    let counts = &report.counts;
    format!(
        "Text: `{}` is on {} {}: {} {}, {} {}, {} {}, {} text only.\n",
        report.target.name,
        counts.text_lines,
        plural(counts.text_lines, "line"),
        counts.site_lines,
        plural(counts.site_lines, "site"),
        counts.definition_lines,
        plural(counts.definition_lines, "definition"),
        counts.import_lines,
        plural(counts.import_lines, "import"),
        counts.text_only_lines,
    )
}

fn site_groups(sites: &[LabeledSite], limit: usize) -> String {
    let mut out = String::new();
    for label in SiteLabel::ALL {
        let group: Vec<&LabeledSite> = sites.iter().filter(|site| site.label == label).collect();
        if group.is_empty() {
            continue;
        }
        let title = match label {
            SiteLabel::Proven => "Proven",
            SiteLabel::Likely => "Likely",
            SiteLabel::Unresolved => "Unresolved",
            SiteLabel::Excluded => "Excluded",
        };
        out.push_str(&format!("{title} ({}):\n", group.len()));
        let shown = limit.min(group.len());
        for labeled in &group[..shown] {
            let site = &labeled.site;
            let line = site.start_line.map(|l| format!(":{l}")).unwrap_or_default();
            out.push_str(&format!(
                "- {} [{}{line}] (kind: {}; {}{})\n",
                caller_text(site),
                site.path,
                site.kind,
                labeled.basis,
                arg_count_text(labeled)
            ));
        }
        if shown < group.len() {
            out.push_str(&fold_line(label, &group[shown..]));
        }
    }
    out
}

/// `; 2 args` after the basis, left out when the basis already starts with the count.
fn arg_count_text(labeled: &LabeledSite) -> String {
    match labeled.arg_count {
        Some(count) => {
            let noun = if count == 1 { "arg" } else { "args" };
            let text = format!("{count} {noun}");
            if labeled.basis.starts_with(&text) {
                String::new()
            } else {
                format!("; {text}")
            }
        }
        None => String::new(),
    }
}

fn fold_line(label: SiteLabel, hidden: &[&LabeledSite]) -> String {
    let (files, named) = files_by_count(hidden.iter().map(|labeled| labeled.site.path.as_str()));
    format!(
        "… {} more {} {} in {files} {}: {named}\n",
        hidden.len(),
        label.as_str(),
        plural(hidden.len(), "site"),
        plural(files, "file"),
    )
}

/// How many files hold `paths`, and the first `FOLD_FILES_NAMED` of them as `path (count)`,
/// most rows first, with `, …` when more files follow.
fn files_by_count<'a>(paths: impl Iterator<Item = &'a str>) -> (usize, String) {
    let mut per_file: BTreeMap<&str, usize> = BTreeMap::new();
    for path in paths {
        *per_file.entry(path).or_default() += 1;
    }
    let mut files: Vec<(&str, usize)> = per_file.into_iter().collect();
    files.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    let named: Vec<String> = files
        .iter()
        .take(FOLD_FILES_NAMED)
        .map(|(path, count)| format!("{path} ({count})"))
        .collect();
    let more = if files.len() > FOLD_FILES_NAMED {
        ", …"
    } else {
        ""
    };
    (files.len(), format!("{}{more}", named.join(", ")))
}

fn override_sections(report: &ReferenceReport, limit: usize) -> String {
    let name = &report.target.display_name;
    let mut out = method_section(
        &format!(
            "Overrides ({}): calls through `{name}` can reach these",
            report.overrides.len()
        ),
        &report.overrides,
        limit,
    );
    out.push_str(&method_section(
        &format!(
            "Overrides of ({}): calls to these can reach `{name}`",
            report.overridden.len()
        ),
        &report.overridden,
        limit,
    ));
    out
}

fn construction_section(rows: &[ConstructionSite], limit: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let shown = limit.min(rows.len());
    let mut per_file: Vec<(&str, Vec<String>)> = Vec::new();
    for row in &rows[..shown] {
        match per_file.last_mut() {
            Some((path, lines)) if *path == row.path => lines.push(row.line.to_string()),
            _ => per_file.push((&row.path, vec![row.line.to_string()])),
        }
    }
    let listed: Vec<String> = per_file
        .iter()
        .map(|(path, lines)| format!("{path}: {}", lines.join(", ")))
        .collect();
    let mut out = format!("Constructions ({}):", rows.len());
    if !listed.is_empty() {
        out.push_str(&format!(" {}", listed.join(" · ")));
    }
    out.push('\n');
    if shown < rows.len() {
        out.push_str(&section_fold(
            rows[shown..].iter().map(|row| row.path.as_str()),
            "construction",
        ));
    }
    out
}

fn variant_section(rows: &[VariantUse], limit: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "Variant uses ({} {}): a match in them may need a new arm\n",
        rows.len(),
        plural(rows.len(), "function")
    );
    let shown = limit.min(rows.len());
    for row in &rows[..shown] {
        let caller = if row.caller.is_empty() {
            "(no enclosing symbol)".to_string()
        } else {
            format!("`{}`", row.caller)
        };
        out.push_str(&format!(
            "- {caller} [{}:{}] names {} of {} {}\n",
            row.path,
            row.line,
            row.variants_used,
            row.variants_total,
            plural(row.variants_total, "variant"),
        ));
    }
    if shown < rows.len() {
        out.push_str(&section_fold(
            rows[shown..].iter().map(|row| row.path.as_str()),
            "function",
        ));
    }
    out
}

fn registration_section(rows: &[Registration], limit: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = format!("Registrations ({}):\n", rows.len());
    let shown = limit.min(rows.len());
    for row in &rows[..shown] {
        out.push_str(&format!(
            "- {} [{}:{}]{}\n",
            registration_label(row),
            row.path,
            row.line,
            if row.by_name { " (by name)" } else { "" }
        ));
    }
    if shown < rows.len() {
        out.push_str(&section_fold(
            rows[shown..].iter().map(|row| row.path.as_str()),
            "registration",
        ));
    }
    out
}

fn registration_label(row: &Registration) -> String {
    if !row.pattern_id.contains(".route.") {
        return row.pattern_id.clone();
    }
    let template = row.template.as_deref().unwrap_or(&row.pattern_id);
    match row.verb.as_deref() {
        Some(verb) => format!("route {verb} {template}"),
        None => format!("route {template}"),
    }
}

fn section_fold<'a>(hidden: impl ExactSizeIterator<Item = &'a str>, word: &str) -> String {
    let count = hidden.len();
    section_fold_with_count(hidden, count, word)
}

fn section_fold_with_count<'a>(
    hidden: impl Iterator<Item = &'a str>,
    count: usize,
    word: &str,
) -> String {
    let (files, named) = files_by_count(hidden);
    format!(
        "… {count} more {} in {files} {}: {named}\n",
        plural(count, word),
        plural(files, "file"),
    )
}

fn test_section(rows: &[CallingTest], limit: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = format!("Tests that call or use it ({}):\n", rows.len());
    let shown = limit.min(rows.len());
    for row in &rows[..shown] {
        let run = row
            .command
            .as_ref()
            .map(|command| format!(" run: {command}"))
            .unwrap_or_default();
        let name_only = if row.name_only { " (name only)" } else { "" };
        out.push_str(&format!(
            "- `{}` [{}:{}]{name_only}{run}\n",
            on_one_line(&row.name),
            row.path,
            row.line
        ));
    }
    if shown < rows.len() {
        out.push_str(&section_fold(
            rows[shown..].iter().map(|row| row.path.as_str()),
            "test",
        ));
    }
    out
}

/// `text` with each control character and Unicode line separator written as an escape (`\n`,
/// `\t`), so a test name never breaks a line of the report.
fn on_one_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') {
                c.escape_default().to_string()
            } else {
                c.to_string()
            }
        })
        .collect()
}

fn method_section(title: &str, rows: &[OverrideRow], limit: usize) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut out = format!("{title}\n");
    let shown = limit.min(rows.len());
    for row in &rows[..shown] {
        out.push_str(&format!(
            "- `{}.{}` ({}:{})\n",
            row.owner, row.name, row.path, row.line
        ));
    }
    if shown < rows.len() {
        out.push_str(&section_fold(
            rows[shown..].iter().map(|row| row.path.as_str()),
            "method",
        ));
    }
    out
}

fn text_only_section(report: &ReferenceReport, limit: usize) -> String {
    if report.text_only.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "Text only ({}): docs, comments, strings, or code the index did not record\n",
        report.counts.text_only_lines
    );
    out.push_str(&text_only_window(report, 0, limit));
    out
}

/// The text-only lines `start..start + limit` in report order, one row per file, and a fold line
/// that counts the lines after them by file.
fn text_only_window(report: &ReferenceReport, start: usize, limit: usize) -> String {
    let total = report
        .text_only
        .iter()
        .map(|file| file.lines.len())
        .sum::<usize>();
    let end = start + limit.min(total.saturating_sub(start));
    let mut per_file: Vec<(&str, Vec<usize>)> = Vec::new();
    let page = report
        .text_only
        .iter()
        .flat_map(|file| {
            file.lines
                .iter()
                .map(move |line| (file.path.as_str(), line))
        })
        .skip(start)
        .take(end.saturating_sub(start));
    for (path, line) in page {
        match per_file.last_mut() {
            Some((previous, lines)) if *previous == path => lines.push(*line),
            _ => per_file.push((path, vec![*line])),
        }
    }
    let mut out = String::new();
    for (path, lines) in per_file {
        let lines = lines
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("  {path}: {lines}\n"));
    }
    if end < total {
        let remainder = report
            .text_only
            .iter()
            .flat_map(|file| file.lines.iter().map(move |_| file.path.as_str()))
            .skip(end);
        out.push_str(&section_fold_with_count(remainder, total - end, "line"));
    }
    out
}

pub(crate) fn displayed_symbol_name(name: &str, enclosing_symbol_name: Option<&str>) -> String {
    if let Some(enclosing_symbol_name) = enclosing_symbol_name {
        format!("<lambda> in {enclosing_symbol_name}")
    } else {
        name.to_string()
    }
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

/// A doc comment line without its comment marker (`///`, `/**`, ` * `, `#`, `--`) or a
/// `<summary>` tag, which julie keeps in the raw comment text.
fn doc_line_text(line: &str) -> &str {
    let mut text = line.trim();
    for marker in [
        "///", "//!", "//", "/**", "/*!", "/*", "##", "#'", "#", "--", "*", "'''", "'",
    ] {
        if let Some(rest) = text.strip_prefix(marker) {
            text = rest.trim_start();
            break;
        }
    }
    for tag in ["<summary>", "</summary>"] {
        text = text.strip_prefix(tag).unwrap_or(text).trim_start();
    }
    for suffix in ["*/", "</summary>"] {
        text = text.strip_suffix(suffix).unwrap_or(text).trim_end();
    }
    text
}

/// The byte index of the period that ends the first sentence: a `. ` outside parentheses and not
/// before a lowercase word, so `(e.g. route)` and `e.g. route` do not end one.
fn sentence_end(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            '.' if depth == 0 => {
                let rest = &text[index + 1..];
                if rest.starts_with(' ') && !rest[1..].starts_with(|next: char| next.is_lowercase())
                {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The first sentence of a doc comment's first paragraph, on one line. A paragraph with no
/// sentence end is cut at 200 characters with `…`.
fn doc_summary(doc: &str) -> Option<String> {
    let paragraph = doc
        .lines()
        .map(doc_line_text)
        .skip_while(|line| line.is_empty())
        .take_while(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let summary = match sentence_end(&paragraph) {
        Some(end) => paragraph[..=end].to_string(),
        None if paragraph.chars().count() > 200 => {
            format!("{}…", paragraph.chars().take(200).collect::<String>())
        }
        None => paragraph,
    };
    (!summary.is_empty()).then_some(summary)
}

pub fn format_find_symbol_results(
    query: &str,
    name_rows: &[Symbol],
    ranked_rows: &[SymbolSearchResult],
    limit: usize,
) -> String {
    let mut out = String::new();
    if !name_rows.is_empty() {
        out.push_str(&format_name_rows(query, name_rows, limit));
    }
    if !ranked_rows.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&format_ranked_rows(query, ranked_rows, limit));
    }
    if out.is_empty() {
        out.push_str(&format!("No symbols found matching \"{query}\".\n"));
    }
    out
}

/// The routes and configuration keys a lookup names: one row each, as
/// ``- route `GET /users/{id}` (path:line) -> handler`` or ``- config `a.b.c` (path:line)``.
pub fn format_lookup_facts(
    query: &str,
    facts: &[crate::models::LookupFact],
    found: usize,
) -> String {
    let routes = facts.iter().filter(|fact| fact.kind == "route").count();
    let keys = facts.len() - routes;
    let mut parts = Vec::new();
    if routes > 0 {
        parts.push(format!("{routes} {}", plural(routes, "route")));
    }
    if keys > 0 {
        parts.push(format!("{keys} config {}", plural(keys, "key")));
    }
    let mut out = format!("Found {} matching \"{query}\":\n\n", parts.join(" and "));
    for fact in facts {
        out.push_str(&format!(
            "- {} `{}` ({}:{})",
            fact.kind, fact.key, fact.path, fact.line
        ));
        if let Some(handler) = &fact.handler {
            out.push_str(&format!(" -> {handler}"));
        }
        out.push('\n');
    }
    if found > facts.len() {
        out.push_str(&format!(
            "[Showing {} of {found} routes and config keys; narrow the path or raise limit.]\n",
            facts.len()
        ));
    }
    out
}

fn format_name_rows(query: &str, exact_matches: &[Symbol], limit: usize) -> String {
    let dotted = query.replace("::", ".");
    let is_exact = |s: &Symbol| s.name == dotted || s.name.ends_with(&format!(".{dotted}"));
    let named_exactly = exact_matches.iter().filter(|s| is_exact(s)).count();
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
        if let Some(doc) = s.doc_comment.as_deref().and_then(doc_summary) {
            out.push_str(&format!("  Doc: {doc}\n"));
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
}

fn format_ranked_rows(query: &str, fts_matches: &[SymbolSearchResult], limit: usize) -> String {
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
        } else if let Some(doc) = s.doc_comment.as_deref().and_then(doc_summary) {
            out.push_str(&format!("  Doc: {doc}\n"));
        }
    }
    if fts_matches.len() >= limit {
        out.push_str(&cap_notice(fts_matches.len(), limit));
    }
    out
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
        let details = format!("{}{}", qt_property_details(f), flask_route_details(f));
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

/// The endpoint of a Flask route and the view of an `add_url_rule` call. A rule with no view, such
/// as `add_url_rule("/", endpoint="index")`, only names an endpoint for `url_for`.
fn flask_route_details(fact: &crate::models::StructuralFact) -> String {
    if !fact.pattern_id.starts_with("flask.route") {
        return String::new();
    }
    let Some(metadata) = fact.metadata.as_ref() else {
        return String::new();
    };
    let text = |key: &str| metadata.get(key).and_then(|v| v.as_str());
    let mut out = String::new();
    if let Some(endpoint) = text("endpoint") {
        out.push_str(&format!(", endpoint: {endpoint}"));
    }
    if text("api_style") == Some("call_routing") {
        match text("view_target") {
            Some(view) => out.push_str(&format!(", view: {view}")),
            None => out.push_str(", no view function"),
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
        } else if let Some(doc) = s.doc_comment.as_deref().and_then(doc_summary) {
            out.push_str(&format!("  Doc: {doc}\n"));
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
    if e.field_diversity_bonus != 0.0 {
        line.push_str(&format!(" + field coverage {:.1}", e.field_diversity_bonus));
    }
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

pub const DEFAULT_EDIT_PAGE_SIZE: usize = 12;
const EDIT_SITES_LISTED: usize = DEFAULT_EDIT_PAGE_SIZE;
const EDIT_TESTS_LISTED: usize = 8;
pub const STOP_SITES_LISTED: usize = 15;

/// The post-edit answer: the base, each symbol whose shape changed with its sites split by
/// whether the diff reached them, the tests to run, and the blind spots of the index.
pub fn format_edit_check(check: &EditCheck) -> String {
    let source = match check.source {
        BaseSource::Given => "given",
        BaseSource::Session => "session base",
        BaseSource::Head => "HEAD",
    };
    let mut out = String::new();
    if check.changes.is_empty() {
        out.push_str(&format!(
            "check_edit: working tree against {} ({source}): no symbol changed shape.\n",
            check.rev
        ));
        push_edit_cuts(&mut out, check);
        return out;
    }
    out.push_str(&format!(
        "check_edit: working tree against {} ({source}), {} {} changed\n{} shape {}:\n",
        check.rev,
        check.files_changed,
        plural(check.files_changed, "file"),
        check.changes.len(),
        plural(check.changes.len(), "change"),
    ));
    for change in &check.changes {
        push_shape_change(&mut out, change);
    }
    push_edit_cuts(&mut out, check);
    if !check.tests.is_empty() {
        let shown = check.tests.len().min(EDIT_TESTS_LISTED);
        out.push_str(&format!(
            "Tests to run: {}",
            check.tests[..shown].join("; ")
        ));
        let found = check.tests_found.max(check.tests.len());
        if shown < found {
            out.push_str(&format!("; … {} more", found - shown));
        }
        out.push('\n');
    }
    if !check.tests_without_command.is_empty() {
        let shown = check.tests_without_command.len().min(EDIT_TESTS_LISTED);
        let places: Vec<String> = check.tests_without_command[..shown]
            .iter()
            .map(test_place)
            .collect();
        out.push_str(&format!("Tests with no run command: {}", places.join("; ")));
        let found = check
            .tests_without_command_found
            .max(check.tests_without_command.len());
        if shown < found {
            out.push_str(&format!("; … {} more", found - shown));
        }
        out.push('\n');
    }
    push_tests_incomplete(&mut out, check);
    out.push_str(
        "Not checked: calls inside macro arguments, callback invocation contracts. Indexed callable values are advisory dependencies.\n",
    );
    out
}

/// Format one offset window for each folded edit-check section.
pub fn format_edit_check_page(check: &EditCheck, offset: usize, limit: usize) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    if limit == 0 {
        return ZERO_LIMIT_NOTICE.to_string();
    }
    if offset == 0 && limit == DEFAULT_EDIT_PAGE_SIZE {
        return format_edit_check(check);
    }
    let source = match check.source {
        BaseSource::Given => "given",
        BaseSource::Session => "session base",
        BaseSource::Head => "HEAD",
    };
    let mut out = String::new();
    if check.changes.is_empty() {
        out.push_str(&format!(
            "check_edit: working tree against {} ({source}): no symbol changed shape.\n",
            check.rev
        ));
        push_edit_cuts(&mut out, check);
        return out;
    }
    out.push_str(&format!(
        "check_edit: working tree against {} ({source}), {} {} changed\n{} shape {}:\n",
        check.rev,
        check.files_changed,
        plural(check.files_changed, "file"),
        check.changes.len(),
        plural(check.changes.len(), "change"),
    ));
    for change in &check.changes {
        push_shape_change_page(&mut out, change, offset, limit);
    }
    push_edit_cuts(&mut out, check);
    if !check.tests.is_empty() {
        let (start, end) = page_bounds(check.tests.len(), offset, limit);
        let found = check.tests_found.max(check.tests.len());
        if found > check.tests.len() {
            out.push_str(&format!(
                "Tests to run ({found} found, {} kept):\n",
                check.tests.len()
            ));
        } else {
            out.push_str(&format!("Tests to run ({}):\n", check.tests.len()));
        }
        page_caption(&mut out, check.tests.len(), start, end, offset);
        if start < end {
            out.push_str(&check.tests[start..end].join("; "));
            out.push('\n');
        }
    }
    if !check.tests_without_command.is_empty() {
        let kept = check.tests_without_command.len();
        let (start, end) = page_bounds(kept, offset, limit);
        let found = check.tests_without_command_found.max(kept);
        if found > kept {
            out.push_str(&format!(
                "Tests with no run command ({found} found, {kept} kept):\n"
            ));
        } else {
            out.push_str(&format!("Tests with no run command ({kept}):\n"));
        }
        page_caption(&mut out, kept, start, end, offset);
        if start < end {
            let places: Vec<String> = check.tests_without_command[start..end]
                .iter()
                .map(test_place)
                .collect();
            out.push_str(&places.join("; "));
            out.push('\n');
        }
    }
    push_tests_incomplete(&mut out, check);
    out.push_str(
        "Not checked: calls inside macro arguments, callback invocation contracts. Indexed callable values are advisory dependencies.\n",
    );
    out
}

/// A test that has no run command, as `` `name` [path:line] ``.
fn test_place(test: &CallingTest) -> String {
    format!(
        "`{}` [{}:{}]",
        on_one_line(&test.name),
        test.path,
        test.line
    )
}

fn push_tests_incomplete(out: &mut String, check: &EditCheck) {
    if check.tests_incomplete.is_empty() {
        return;
    }
    let lead = if check.tests.is_empty() && check.tests_without_command.is_empty() {
        "No test reaches the changed symbols through the index"
    } else {
        "Tests to run may be incomplete"
    };
    out.push_str(&format!("{lead}: {}. ", check.tests_incomplete.join("; ")));
    match &check.broader_test_command {
        Some(command) => out.push_str(&format!("Broader test command: `{command}`\n")),
        None => out.push_str("Run the full test suite.\n"),
    }
}

fn push_shape_change_page(out: &mut String, change: &ShapeChange, offset: usize, limit: usize) {
    out.push_str(&format!(
        "  {} `{}` ({}): {}\n",
        change.kind, change.display, change.path, change.change
    ));
    if !change.in_diff.is_empty() {
        let (start, end) = page_bounds(change.in_diff.len(), offset, limit);
        out.push_str(&format!("    In your diff ({}):\n", change.in_diff.len()));
        page_caption(out, change.in_diff.len(), start, end, offset);
        for site in &change.in_diff[start..end] {
            out.push_str(&format!("      {}:{}\n", site.path, site.line));
        }
    }
    for (label, sites) in [("proven", &change.proven), ("likely", &change.likely)] {
        if sites.is_empty() {
            continue;
        }
        let (start, end) = page_bounds(sites.len(), offset, limit);
        out.push_str(&format!(
            "    Not in your diff, {label} ({}):\n",
            sites.len()
        ));
        page_caption(out, sites.len(), start, end, offset);
        for site in &sites[start..end] {
            out.push_str(&edit_site_row(site));
        }
    }
    if change.unresolved > 0 {
        out.push_str(&format!(
            "    Unresolved outside the diff: {}\n",
            change.unresolved
        ));
    }
}

/// The reason a Stop hook gives for a check: how many `proven` sites outside the diff still use
/// the old shape of which symbols, then the first of them. `None` when no change that
/// [`blocks_a_stop`] has such a site.
pub fn format_stop_reason(check: &EditCheck) -> Option<String> {
    let rows = stop_rows(check);
    if rows.is_empty() {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for (_, change) in &rows {
        let name = format!("`{}`", change.display);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let mut out = format!(
        "Before you finish: {} sites still use the old shape of {}.\n",
        rows.len(),
        names.join(", ")
    );
    for (site, change) in rows.iter().take(STOP_SITES_LISTED) {
        out.push_str(&format!(
            "{}:{}  {}: {}\n",
            site.path, site.line, change.display, change.change
        ));
    }
    out.push_str("Edit them or say why they need no change.");
    Some(out)
}

fn stop_rows(check: &EditCheck) -> Vec<(&EditSite, &ShapeChange)> {
    let mut rows: Vec<(&EditSite, &ShapeChange)> = Vec::new();
    for change in check.changes.iter().filter(|change| blocks_a_stop(change)) {
        rows.extend(change.proven.iter().map(|site| (site, change)));
    }
    rows.sort_by(|a, b| {
        (&a.0.path, a.0.line, &a.1.display).cmp(&(&b.0.path, b.0.line, &b.1.display))
    });
    rows
}

/// The `proven` sites that block a stop, in the order its reason lists them.
pub fn stop_sites(check: &EditCheck) -> Vec<&EditSite> {
    stop_rows(check).into_iter().map(|(site, _)| site).collect()
}

/// A `signature changed` row is often a type change that every call still fits (a nullable
/// `?`, a type alias, a return type), and JavaScript ignores the argument count. On clean
/// commits of real projects both blocked stops that needed no edit, so neither blocks a stop.
fn blocks_a_stop(change: &ShapeChange) -> bool {
    let javascript = [".js", ".jsx", ".mjs", ".cjs"]
        .iter()
        .any(|extension| change.path.ends_with(extension));
    !change.change.starts_with("signature changed")
        && !(javascript && change.change.starts_with("parameters"))
}

fn push_edit_cuts(out: &mut String, check: &EditCheck) {
    for cut in &check.cuts {
        out.push_str(&format!("Cut: {cut}\n"));
    }
}

fn push_shape_change(out: &mut String, change: &ShapeChange) {
    out.push_str(&format!(
        "  {} `{}` ({}): {}\n",
        change.kind, change.display, change.path, change.change
    ));
    if !change.in_diff.is_empty() {
        let shown = change.in_diff.len().min(EDIT_SITES_LISTED);
        let places: Vec<String> = change.in_diff[..shown]
            .iter()
            .map(|site| format!("{}:{}", site.path, site.line))
            .collect();
        out.push_str(&format!(
            "    In your diff ({}): {}",
            change.in_diff.len(),
            places.join(", ")
        ));
        if shown < change.in_diff.len() {
            out.push_str(&format!(", … {} more", change.in_diff.len() - shown));
        }
        out.push('\n');
    }
    for (label, sites) in [("proven", &change.proven), ("likely", &change.likely)] {
        if sites.is_empty() {
            continue;
        }
        out.push_str(&format!(
            "    Not in your diff, {label} ({}):\n",
            sites.len()
        ));
        for site in &sites[..sites.len().min(EDIT_SITES_LISTED)] {
            out.push_str(&edit_site_row(site));
        }
        if sites.len() > EDIT_SITES_LISTED {
            out.push_str(&format!(
                "      … {} more {label}\n",
                sites.len() - EDIT_SITES_LISTED
            ));
        }
    }
    if change.unresolved > 0 {
        out.push_str(&format!(
            "    Unresolved outside the diff: {}\n",
            change.unresolved
        ));
    }
}

fn edit_site_row(site: &EditSite) -> String {
    let caller = if site.caller.is_empty() {
        "(no enclosing symbol)".to_string()
    } else {
        format!("`{}`", site.caller)
    };
    let note = site
        .note
        .as_deref()
        .map(|note| format!("  {note}"))
        .unwrap_or_default();
    format!("      {}:{}  {caller}{note}\n", site.path, site.line)
}

/// The warning for the symbols where the walk stopped because no indexed code calls them: the
/// first three on the first page, else the rows of the page window.
fn open_ends_text(open_ends: &[OpenEnd], page: Option<(usize, usize)>) -> String {
    const FIRST_PAGE: usize = 3;
    const INTRO: &str = "No indexed code calls these symbols, so the walk stopped at them. The runtime, a framework, a library, or a call that the index cannot tie may call them, or nothing does. The list above does not show the tests that reach the target through them:\n";
    if open_ends.is_empty() {
        return String::new();
    }
    let (start, end) = match page {
        Some((offset, limit)) => page_bounds(open_ends.len(), offset, limit),
        None => (0, open_ends.len().min(FIRST_PAGE)),
    };
    let mut out = match page {
        Some((offset, _)) if start == end => {
            return format!(
                "### Open Ends (0 shown at offset {offset}; {} found)\nNo open ends at this offset.\n\n",
                open_ends.len()
            );
        }
        Some((offset, _)) => format!(
            "### Open Ends ({} shown at offset {offset}; {} found)\n{INTRO}",
            end - start,
            open_ends.len()
        ),
        None => format!("### Open Ends\n{INTRO}"),
    };
    for end in &open_ends[start..end] {
        let name = match &end.owner {
            Some(owner) => format!("{owner}.{}", end.name),
            None => end.name.clone(),
        };
        out.push_str(&format!(
            "  - {} `{name}` ({}:{})\n",
            end.kind, end.path, end.line
        ));
    }
    if open_ends.len() > end {
        out.push_str(&format!(
            "  - {} more; pass offset {end} to list them.\n",
            open_ends.len() - end
        ));
    }
    out.push('\n');
    out
}

/// Format blast radius and likely test targets into token-dense markdown.
pub fn format_blast_radius(result: &BlastRadiusResult) -> String {
    format_blast_radius_with_page(result, 0, usize::MAX, false, false)
}

/// Format a bounded page of impact and test rows.
pub fn format_blast_radius_page(
    result: &BlastRadiusResult,
    offset: usize,
    limit: usize,
    include_low_signal: bool,
) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    if limit == 0 {
        return ZERO_LIMIT_NOTICE.to_string();
    }
    if offset == 0 && !include_low_signal {
        return format_blast_radius(result);
    }
    format_blast_radius_with_page(result, offset, limit, include_low_signal, true)
}

fn format_blast_radius_with_page(
    result: &BlastRadiusResult,
    offset: usize,
    limit: usize,
    include_low_signal: bool,
    paged: bool,
) -> String {
    if result.seed_type == "none" {
        return "No uncommitted changes detected in git working tree. Pass a 'symbol' or 'file' parameter to analyze blast radius.".to_string();
    }

    let (test_start, test_end) = if paged {
        page_bounds(result.likely_tests.len(), offset, limit)
    } else {
        (0, result.likely_tests.len())
    };
    let test_rows = &result.likely_tests[test_start..test_end];
    let (impact_start, impact_end) = if paged {
        page_bounds(result.impacted_symbols.len(), offset, limit)
    } else {
        (0, result.impacted_symbols.len())
    };
    let impact_candidates = &result.impacted_symbols[impact_start..impact_end];

    let mut out = String::new();
    let seed_label = if result.seed_type == "file" {
        format!("Files: {}", result.seeds.join(", "))
    } else if result.seed_type == "symbol" {
        format!("Symbol: {}", result.seeds.join(", "))
    } else {
        format!("Seeds: {}", result.seeds.join(", "))
    };

    out.push_str(&format!("## Blast Radius & Test Impact ({seed_label})\n\n"));
    let coverage = coverage_gap_line(result.coverage_gap.as_ref());
    if !coverage.is_empty() {
        out.push_str(&coverage);
        out.push('\n');
    }
    if paged {
        out.push_str(&format!(
            "Page offset {offset}, limit {limit}; each section pages independently. Restart at offset 0 after an index change.\n\n"
        ));
    }

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
    if result.likely_tests_truncated && !paged {
        out.push_str(&shown_of(
            result.likely_tests.len(),
            result.likely_tests_found,
            "likely tests",
        ));
    }
    if result.impacted_symbols_truncated && !paged {
        out.push_str(&shown_of(
            result.impacted_symbols.len(),
            result.impacted_symbols_found,
            "impacted symbols",
        ));
    }
    if result.traversal_ceiling_reached {
        out.push_str("Traversal stopped at the 200-row discovery ceiling; narrow the target because increasing limit cannot raise this ceiling.\n\n");
    }

    if !test_rows.is_empty() {
        if paged {
            out.push_str(&format!(
                "### Likely Tests to Run ({} shown at offset {}; {} found)\n",
                test_rows.len(),
                offset,
                result.likely_tests_found
            ));
        } else {
            out.push_str(&format!(
                "### Likely Tests to Run ({} returned)\n",
                result.likely_tests.len()
            ));
        }

        // Preserve ranked row order; collecting every row by file can put lower-ranked tests first.
        let mut previous_path = None;
        for t in test_rows {
            if previous_path != Some(t.path.as_str()) {
                out.push_str(&format!("{}:\n", t.path));
                previous_path = Some(t.path.as_str());
            }
            out.push_str(&format!(
                "  - `{}` [line {}] ({})\n",
                t.name, t.line, t.reason
            ));
        }
        if let (Some(depth), Some(more)) = (result.stopped_at_depth, result.tests_past_depth)
            && more > 0
        {
            let at_least = if result.tests_past_depth_cut {
                "at least "
            } else {
                ""
            };
            out.push_str(&format!(
                "The walk stopped at depth {depth}. A `depth` of {} lists {at_least}{more} more {}.\n",
                crate::queries::MAX_BLAST_DEPTH,
                plural(more, "test")
            ));
        }

        out.push('\n');
    } else if paged && result.likely_tests_found > 0 {
        out.push_str(&format!(
            "### Likely Tests to Run (0 shown at offset {}; {} found)\nNo tests at this offset.\n\n",
            offset, result.likely_tests_found
        ));
    } else if result.likely_tests_truncated || result.traversal_ceiling_reached {
        out.push_str("### Likely Tests to Run (0 returned)\n\n");
    } else if let Some(depth) = result.stopped_at_depth {
        let largest = crate::queries::MAX_BLAST_DEPTH;
        let advice = match result.tests_past_depth {
            Some(0) => {
                format!("No test reaches it within depth {largest}. Run the full test suite.")
            }
            Some(more) => format!(
                "No test reaches it within depth {depth}. A `depth` of {largest} lists {}{more} {}.",
                if result.tests_past_depth_cut {
                    "at least "
                } else {
                    ""
                },
                plural(more, "test")
            ),
            None => format!(
                "No test reaches it within depth {depth}; a larger `depth` may find some. Otherwise run the full test suite."
            ),
        };
        out.push_str(&format!(
            "### Likely Tests to Run (0 returned)\n{advice}\n\n"
        ));
    } else {
        out.push_str(
            "### Likely Tests to Run (0 returned)\nNo test reaches it through the index. Run the full test suite.\n\n",
        );
    }

    if let Some(count) = result.report_symbols_unchecked {
        out.push_str(&format!(
            "find_references did not check {count} {} of the target within the time limit, so tests that only {} reach may be missing.\n\n",
            plural(count, "symbol"),
            if count == 1 { "it" } else { "they" }
        ));
    }
    if let Some(count) = result.report_symbols_failed {
        out.push_str(&format!(
            "find_references failed for {count} {} of the target, so the callers and tests that only {} report ties may be missing.\n\n",
            plural(count, "symbol"),
            if count == 1 { "its" } else { "their" }
        ));
    }
    if let Some(count) = result.report_sites_unchecked {
        out.push_str(&format!(
            "find_references did not check {count} {} within the time limit, so callers and tests that only {} would tie may be missing.\n\n",
            plural(count, "site"),
            if count == 1 { "it" } else { "they" }
        ));
    }
    out.push_str(&open_ends_text(
        &result.open_ends,
        paged.then_some((offset, limit)),
    ));

    if !impact_candidates.is_empty() {
        let mut visible = Vec::new();
        let mut low_signal_count = 0;
        for s in impact_candidates {
            if matches!(s.kind.as_str(), "import" | "module" | "namespace") && !include_low_signal {
                low_signal_count += 1;
            } else {
                visible.push(s);
            }
        }

        if paged {
            out.push_str(&format!(
                "### Downstream Impact ({} shown at offset {}; {} found)\n",
                visible.len(),
                offset,
                result.impacted_symbols_found
            ));
        } else {
            out.push_str(&format!(
                "### Downstream Impact ({} returned)\n",
                result.impacted_symbols.len()
            ));
        }

        if visible.is_empty() && low_signal_count > 0 {
            let row_word = if low_signal_count == 1 {
                "row (import/module)"
            } else {
                "rows (imports/modules)"
            };
            out.push_str(&format!(
                "All impacted symbols are imports/modules; {low_signal_count} low-signal {row_word} hidden; pass include_low_signal to show them.\n"
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
                        let name =
                            displayed_symbol_name(&s.name, s.enclosing_symbol_name.as_deref());
                        let reason = s
                            .reason
                            .as_deref()
                            .map(|reason| format!(" ({reason})"))
                            .unwrap_or_default();
                        out.push_str(&format!(
                            "  - [depth {}] {} `{}` [line {}]{reason}\n",
                            s.depth, s.kind, name, s.line
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
                    "... {low_signal_count} low-signal {row_word} hidden; pass include_low_signal to show them.\n"
                ));
            }
        }
        if let Some(report) = &result.callers_report
            && report.unresolved > 0
        {
            out.push_str(&untied_sites(report, " more"));
        }
    } else if paged && result.impacted_symbols_found > 0 {
        out.push_str(&format!(
            "### Downstream Impact (0 shown at offset {}; {} found)\nNo symbols at this offset.\n",
            offset, result.impacted_symbols_found
        ));
    } else if result.impacted_symbols_truncated || result.traversal_ceiling_reached {
        out.push_str("### Downstream Impact (0 returned)\n");
    } else {
        out.push_str("### Downstream Impact (0 returned)\n");
        match &result.callers_report {
            Some(report) if report.proven + report.likely > 0 => {
                out.push_str(
                    "Every caller that find_references ties to this definition is a test.\n",
                );
                if report.unresolved > 0 {
                    out.push_str(&untied_sites(report, " more"));
                }
            }
            Some(report) if report.unresolved > 0 => out.push_str(&untied_sites(report, "")),
            Some(report) if report.sites > 0 => out.push_str(&format!(
                "find_references ties all {} {} of the name to other definitions.\n",
                report.sites,
                plural(report.sites, "site")
            )),
            _ => out.push_str("No downstream callers found within depth.\n"),
        }
    }

    out
}

/// Format blast radius with the broader suite command selected from this report's paths.
pub fn format_blast_radius_report(
    root: &Path,
    result: &BlastRadiusResult,
    offset: usize,
    limit: usize,
    include_low_signal: bool,
) -> String {
    if let Some(error) = invalid_page_window(offset, limit) {
        return error;
    }
    let mut out = format_blast_radius_page(result, offset, limit, include_low_signal);
    let needs_broader_suite = result.likely_tests.is_empty()
        || result.likely_tests_truncated
        || result.traversal_ceiling_reached
        || result.stopped_at_depth.is_some()
        || result.report_symbols_unchecked.is_some()
        || result.report_symbols_failed.is_some()
        || result.report_sites_unchecked.is_some()
        || !result.open_ends.is_empty();
    if !needs_broader_suite || result.seed_type == "none" {
        return out;
    }

    let mut paths: Vec<&str> = result
        .likely_tests
        .iter()
        .map(|test| test.path.as_str())
        .chain(
            result
                .impacted_symbols
                .iter()
                .map(|symbol| symbol.path.as_str()),
        )
        .chain(result.open_ends.iter().map(|end| end.path.as_str()))
        .collect();
    if result.seed_type == "file" {
        paths.extend(result.seeds.iter().map(String::as_str));
    } else if result.seed_type == "mixed" {
        paths.clear();
    }
    paths.sort_unstable();
    paths.dedup();
    let mut finder = crate::test_commands::CommandFinder::new(root);
    let mut commands: BTreeMap<String, usize> = BTreeMap::new();
    let mut uncovered: Vec<&str> = Vec::new();
    for path in &paths {
        match finder.broader_test_command(Some(path)) {
            Some(command) => *commands.entry(command).or_default() += 1,
            None => uncovered.push(path),
        }
    }
    out.push_str("\n### Broader Test Scope\n");
    if commands.is_empty() {
        out.push_str(
            "Broader scope: full project test suite.\nNo supported runner configuration covers the reported paths; run the full suite manually.\n",
        );
        return out;
    }
    if commands.len() == 1 && uncovered.is_empty() {
        let command = commands.keys().next().expect("one command");
        out.push_str(&format!("Broader test command: `{command}`\n"));
    } else {
        out.push_str("Broader test commands, each for the reported paths its runner covers:\n");
        for (command, count) in &commands {
            out.push_str(&format!(
                "  - `{command}` ({count} {})\n",
                plural(*count, "path")
            ));
        }
    }
    if !uncovered.is_empty() {
        let shown: Vec<String> = uncovered
            .iter()
            .take(3)
            .map(|path| format!("`{path}`"))
            .collect();
        let more = uncovered.len().saturating_sub(shown.len());
        out.push_str(&format!(
            "No supported runner configuration covers {} reported {}: {}{}; check {} by hand.\n",
            uncovered.len(),
            plural(uncovered.len(), "path"),
            shown.join(", "),
            if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            },
            if uncovered.len() == 1 { "it" } else { "them" },
        ));
    }
    if cfg!(windows) {
        out.push_str(
            "Command syntax uses POSIX shell quoting; translate it before running in a Windows shell.\n",
        );
    }
    out
}

fn untied_sites(report: &CallersReportSites, more: &str) -> String {
    let count = report.unresolved;
    format!(
        "The index could not tie {count}{more} {} of the name to this definition; find_references lists {}.\n",
        plural(count, "site"),
        if count == 1 { "it" } else { "them" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        CallersReportSites, ImpactedSymbol, LabeledSite, ReferenceCounts, ReferenceReport,
        ReferenceTarget, SiteLabel, TestTarget, TextOnlyLines, TextScanSummary,
    };

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
    fn format_structural_facts_names_the_endpoint_and_view_of_a_flask_route() {
        let route = |metadata| crate::models::StructuralFact {
            pattern_id: "flask.route.v1".into(),
            metadata: Some(metadata),
            ..structural_fact(Some("GET /"))
        };
        let alias = route(serde_json::json!({"api_style": "call_routing", "endpoint": "index"}));
        let rule = route(serde_json::json!({"api_style": "call_routing", "view_target": "ping"}));
        let decorated = route(serde_json::json!({"api_style": "decorator_routing"}));

        let output = format_structural_facts(&[alias, rule, decorated], &[], "route", 30);

        assert!(
            output.contains("(pattern: flask.route.v1, endpoint: index, no view function)"),
            "{output}"
        );
        assert!(
            output.contains("(pattern: flask.route.v1, view: ping)"),
            "{output}"
        );
        assert!(output.contains("(pattern: flask.route.v1)"), "{output}");
    }

    #[test]
    fn doc_summary_ends_at_the_first_sentence_of_the_first_paragraph() {
        assert_eq!(
            doc_summary(
                "The actual WSGI application. This is not implemented in\n:meth:`__call__`."
            )
            .as_deref(),
            Some("The actual WSGI application.")
        );
        assert_eq!(
            doc_summary("\nCreate a runner\nfor tests\n\nMore.").as_deref(),
            Some("Create a runner for tests")
        );
        assert_eq!(
            doc_summary("Find facts by category (e.g. route, query). More.").as_deref(),
            Some("Find facts by category (e.g. route, query).")
        );
        assert_eq!(
            doc_summary("Use a name, e.g. foo. Then more.").as_deref(),
            Some("Use a name, e.g. foo.")
        );
        assert_eq!(
            doc_summary("/// <summary>\n/// Goes to previous page.\n/// </summary>").as_deref(),
            Some("Goes to previous page.")
        );
        assert_eq!(
            doc_summary("/**\n * Render the view.\n *\n * @param x\n */").as_deref(),
            Some("Render the view.")
        );
        assert_eq!(
            doc_summary("# Given a +hash+ returns the settings\n# for the environment.").as_deref(),
            Some("Given a +hash+ returns the settings for the environment.")
        );
        assert_eq!(
            doc_summary("#' A person record.").as_deref(),
            Some("A person record.")
        );
        assert_eq!(doc_summary("  \n"), None);
        assert!(doc_summary(&"word ".repeat(60)).unwrap().ends_with('…'));
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
            enclosing_symbol_name: None,
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

        let mut commented = syms.clone();
        commented[0].doc_comment = Some("/**\n * Performs core work.\n */".into());
        let skeleton = format_file_skeleton("src/lib.rs", &commented, Some(35), 0);
        assert!(skeleton.contains("/// Performs core work.\n"), "{skeleton}");
        assert!(!skeleton.contains("/// /**"), "{skeleton}");
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
                private: 0,
            },
        )]);
        add_path_to_outline(&mut root, "tests/test_basic.py", &symbols, &counts, 3, "");

        let mut out = String::new();
        render_outline_tree(&mut out, &root, "", 0, 3, OUTLINE_NAMES_PER_FILE);

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
        render_outline_tree(&mut out, &root, "", 0, 2, OUTLINE_NAMES_PER_FILE);

        assert!(out.contains("└── flask/ (3 indexed files)"), "{out}");
        assert!(!out.contains("tests/ ("), "{out}");

        let mut one_level = String::new();
        render_outline_tree(&mut one_level, &root, "", 0, 1, 0);

        assert_eq!(
            one_level,
            "├── src/ (3 indexed files)\n└── tests/ (1 indexed file)\n"
        );
    }

    #[test]
    fn a_lookup_qualified_with_double_colons_names_the_member_exactly() {
        let rows = vec![sample_symbol("Flask.handle_user_exception")];

        let out = format_find_symbol_results("Flask::handle_user_exception", &rows, &[], 20);

        assert!(!out.contains("named exactly"), "{out}");
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
    fn lookup_lists_name_rows_before_the_ranked_rows_under_their_own_header() {
        let names = vec![sample_symbol("retry_count")];
        let ranked = vec![SymbolSearchResult {
            symbol: sample_symbol("send_again"),
            score: 7.5,
            snippet: None,
            explain: None,
        }];

        let out = format_find_symbol_results("retry", &names, &ranked, 20);

        let name_header = out.find("Found 1 symbols matching \"retry\"").unwrap();
        let ranked_header = out
            .find("\nNo exact name match; 1 full-text matches for \"retry\":\n\n")
            .unwrap();
        assert!(name_header < out.find("`retry_count`").unwrap(), "{out}");
        assert!(out.find("`retry_count`").unwrap() < ranked_header, "{out}");
        assert!(ranked_header < out.find("`send_again`").unwrap(), "{out}");
    }

    #[test]
    fn every_printed_part_of_the_explain_line_sums_to_the_score() {
        let e = SearchExplain {
            bm25: Some(-3.21),
            branches: vec!["word".into()],
            name_tier: "all".into(),
            name_strength: 6,
            term_score: 24.5,
            field_diversity_bonus: 6.0,
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
            + e.field_diversity_bonus
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
        assert!(line.contains("+ field coverage 6.0"));
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
                field_diversity_bonus: 0.0,
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
                enclosing_symbol_name: None,
                reason: None,
            }],
            traversal_ceiling_reached: false,
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("## Blast Radius & Test Impact (Symbol: do_work)"));
        assert!(formatted.contains("### Likely Tests to Run (1 returned)"));
        assert!(
            formatted.contains("tests/work_test.rs:\n  - `test_do_work` [line 15] (direct caller)")
        );
        assert!(formatted.contains("src/caller.rs:\n  - [depth 1] function `caller_fn` [line 42]"));
    }

    fn empty_scope_result(seed_type: &str, seeds: &[&str]) -> BlastRadiusResult {
        BlastRadiusResult {
            seed_type: seed_type.into(),
            seeds: seeds.iter().map(|seed| (*seed).to_string()).collect(),
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            traversal_ceiling_reached: false,
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
        }
    }

    #[test]
    fn broader_scope_without_target_paths_does_not_discover_a_root_runner() {
        let root = crate::safe_tempdir();
        std::fs::write(
            root.path().join("package.json"),
            r#"{"scripts":{"test":"node --test"}}"#,
        )
        .unwrap();
        let result = empty_scope_result("symbol", &["opaque-current-index-id"]);
        let text = format_blast_radius_report(root.path(), &result, 0, 20, false);
        assert!(!text.contains("Broader test command:"), "{text}");
        assert!(text.contains("run the full suite manually"), "{text}");
    }

    #[test]
    fn broader_scope_seed_file_preserves_the_requested_path() {
        let root = crate::safe_tempdir();
        std::fs::write(root.path().join("pytest.ini"), "[pytest]\n").unwrap();
        let result = empty_scope_result("file", &["python/app.py"]);
        let text = format_blast_radius_report(root.path(), &result, 0, 20, false);
        assert!(text.contains("Broader test command: `pytest`"), "{text}");
    }

    #[test]
    fn broader_scope_seed_mixed_does_not_hide_an_unresolved_symbol() {
        let root = crate::safe_tempdir();
        std::fs::write(root.path().join("pytest.ini"), "[pytest]\n").unwrap();
        let mut result = empty_scope_result("mixed", &["opaque-current-index-id", "python/app.py"]);
        result.open_ends.push(OpenEnd {
            name: "endpoint".into(),
            kind: "function".into(),
            path: "python/app.py".into(),
            line: 1,
            depth: 0,
            owner: None,
        });
        let text = format_blast_radius_report(root.path(), &result, 0, 20, false);
        assert!(!text.contains("Broader test command:"), "{text}");
        assert!(text.contains("run the full suite manually"), "{text}");
    }

    fn result_with_open_ends(count: usize) -> BlastRadiusResult {
        let mut result = empty_scope_result("file", &["src/cli.py"]);
        result.open_ends = (1..=count)
            .map(|line| OpenEnd {
                name: format!("command_{line}"),
                kind: "function".into(),
                path: "src/cli.py".into(),
                line,
                depth: 0,
                owner: None,
            })
            .collect();
        result
    }

    #[test]
    fn the_first_page_names_the_offset_that_lists_the_other_open_ends() {
        let text = format_blast_radius(&result_with_open_ends(5));

        assert!(text.contains("`command_3`"), "{text}");
        assert!(!text.contains("`command_4`"), "{text}");
        assert!(
            text.contains("  - 2 more; pass offset 3 to list them.\n"),
            "{text}"
        );
        assert!(!text.contains("CLI --json"), "{text}");
    }

    #[test]
    fn a_later_page_lists_the_open_ends_at_its_offset() {
        let result = result_with_open_ends(5);

        let text = format_blast_radius_page(&result, 3, 1, false);
        assert!(
            text.contains("### Open Ends (1 shown at offset 3; 5 found)\n"),
            "{text}"
        );
        assert!(!text.contains("`command_3`"), "{text}");
        assert!(text.contains("`command_4`"), "{text}");
        assert!(
            text.contains("  - 1 more; pass offset 4 to list them.\n"),
            "{text}"
        );

        let past_end = format_blast_radius_page(&result, 9, 20, false);
        assert!(
            past_end.contains(
                "### Open Ends (0 shown at offset 9; 5 found)\nNo open ends at this offset.\n"
            ),
            "{past_end}"
        );
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
                enclosing_symbol_name: None,
                reason: None,
            },
            ImpactedSymbol {
                name: "service_fn".into(),
                kind: "function".into(),
                path: "src/service.rs".into(),
                line: 20,
                depth: 1,
                enclosing_symbol_name: None,
                reason: None,
            },
            ImpactedSymbol {
                name: "api_handler".into(),
                kind: "function".into(),
                path: "src/api.rs".into(),
                line: 45,
                depth: 2,
                enclosing_symbol_name: None,
                reason: None,
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
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
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
        assert!(formatted.contains(
            "... 1 low-signal row (import/module) hidden; pass include_low_signal to show them."
        ));
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

    #[test]
    fn skeleton_renders_children_after_one_line_scope_owners() {
        let erlang_owner = Symbol {
            language: "erlang".into(),
            kind: "module".into(),
            signature: Some("-module(conformance);".into()),
            start_line: 1,
            end_line: 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol("conformance")
        };
        let erlang_function = |name: &str, line: usize| Symbol {
            language: "erlang".into(),
            kind: "function".into(),
            signature: Some(format!("{name}() -> ok.")),
            parent_symbol_id: Some("id_conformance".into()),
            start_line: line,
            end_line: line,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol(name)
        };
        let gdscript_owner = Symbol {
            language: "gdscript".into(),
            kind: "class".into(),
            signature: Some("extends Node".into()),
            start_line: 1,
            end_line: 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol("sample")
        };
        let gdscript_method = |name: &str, line: usize| Symbol {
            language: "gdscript".into(),
            kind: "method".into(),
            signature: Some(format!("func {name}() -> int")),
            parent_symbol_id: Some("id_sample".into()),
            start_line: line,
            end_line: line + 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol(name)
        };
        let symbols = vec![
            erlang_owner,
            erlang_function("anchor_erlang", 3),
            erlang_function("caller_erlang", 4),
            gdscript_owner,
            gdscript_method("anchor_gdscript", 2),
            gdscript_method("caller_gdscript", 4),
        ];

        let out = format_file_skeleton("sample", &symbols, None, 0);

        for expected in [
            "anchor_erlang",
            "caller_erlang",
            "anchor_gdscript",
            "caller_gdscript",
        ] {
            assert!(out.contains(expected), "{out}");
        }
    }

    #[test]
    fn skeleton_renders_same_line_xml_and_json_children_missing_from_owner_signatures() {
        let mut xml = vec![
            skeleton_row(
                "project",
                None,
                "module",
                "Project",
                "<Project Sdk=\"Microsoft.NET.Sdk\">",
                (1, 1),
                None,
            ),
            skeleton_row(
                "anchor",
                Some("project"),
                "function",
                "anchor_xml",
                "<Target Name=\"anchor_xml\">",
                (1, 1),
                None,
            ),
            skeleton_row(
                "caller",
                Some("project"),
                "function",
                "caller_xml",
                "<Target Name=\"caller_xml\">",
                (1, 1),
                None,
            ),
        ];
        for symbol in &mut xml {
            symbol.language = "xml".into();
        }
        let xml = format_file_skeleton("Project.csproj", &xml, Some(1), 0);
        assert!(xml.contains("<Target Name=\"anchor_xml\">"), "{xml}");
        assert!(xml.contains("<Target Name=\"caller_xml\">"), "{xml}");

        let mut json = vec![
            skeleton_row(
                "anchor",
                None,
                "module",
                "anchor_json",
                "anchor_json",
                (1, 1),
                None,
            ),
            skeleton_row(
                "value",
                Some("anchor"),
                "variable",
                "value",
                "\"value\": 41",
                (1, 1),
                None,
            ),
        ];
        for symbol in &mut json {
            symbol.language = "json".into();
        }
        let json = format_file_skeleton("sample.json", &json, Some(1), 0);
        assert!(json.contains("\"value\": 41"), "{json}");
    }

    #[test]
    fn skeleton_shows_css_custom_properties_without_nested_style_declarations() {
        let rule = Symbol {
            language: "css".into(),
            kind: "property".into(),
            signature: Some(":root { --accent-color: #ffffff }".into()),
            start_line: 1,
            end_line: 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol(":root")
        };
        let custom_property = Symbol {
            language: "css".into(),
            kind: "property".into(),
            signature: Some("--accent-color: #ffffff".into()),
            parent_symbol_id: Some("id_:root".into()),
            start_line: 1,
            end_line: 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol("--accent-color")
        };
        let style_declaration = Symbol {
            language: "css".into(),
            kind: "property".into(),
            signature: Some("color: var(--accent-color)".into()),
            parent_symbol_id: Some("id_:root".into()),
            start_line: 1,
            end_line: 1,
            body_start_line: None,
            body_end_line: None,
            ..sample_symbol("color")
        };

        let out = format_file_skeleton(
            "sample.css",
            &[rule, custom_property, style_declaration],
            None,
            0,
        );

        assert!(out.contains("--accent-color: #ffffff"), "{out}");
        assert!(!out.contains("color: var"), "{out}");
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
            text.ends_with("### Related Tests:\nNo related test was found in the index; blast_radius may find tests through callers.\n"),
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
                enclosing_symbol_name: None,
                reason: None,
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
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("### Downstream Impact (200 returned)\n"));
    }

    #[test]
    fn blast_radius_does_not_claim_no_results_after_the_traversal_ceiling() {
        let res = BlastRadiusResult {
            seed_type: "file".into(),
            seeds: vec!["src/widget.rs".into()],
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            traversal_ceiling_reached: true,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: None,
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
        };

        let formatted = format_blast_radius(&res);
        assert!(formatted.contains("Likely Tests to Run (0 returned)"));
        assert!(formatted.contains("Downstream Impact (0 returned)"));
        assert!(!formatted.contains("No test reaches it through the index"));
        assert!(!formatted.contains("No downstream callers found within depth"));

        let no_ceiling = BlastRadiusResult {
            traversal_ceiling_reached: false,
            ..res
        };
        assert!(
            format_blast_radius(&no_ceiling)
                .contains("No test reaches it through the index. Run the full test suite.\n")
        );

        let cut_by_depth = BlastRadiusResult {
            stopped_at_depth: Some(2),
            ..no_ceiling
        };
        let formatted = format_blast_radius(&cut_by_depth);
        assert!(formatted.contains(
            "No test reaches it within depth 2; a larger `depth` may find some. Otherwise run the full test suite.\n"
        ));
        assert!(!formatted.contains("through the index"));

        let deeper_tests = BlastRadiusResult {
            tests_past_depth: Some(3),
            tests_past_depth_cut: true,
            ..cut_by_depth.clone()
        };
        assert!(format_blast_radius(&deeper_tests).contains(
            "No test reaches it within depth 2. A `depth` of 5 lists at least 3 tests.\n"
        ));

        let no_deeper_test = BlastRadiusResult {
            tests_past_depth: Some(0),
            ..cut_by_depth
        };
        assert!(
            format_blast_radius(&no_deeper_test)
                .contains("No test reaches it within depth 5. Run the full test suite.\n")
        );
    }

    #[test]
    fn blast_radius_with_callers_report_sites_never_claims_no_callers() {
        let unresolved_only = BlastRadiusResult {
            seed_type: "symbol".into(),
            seeds: vec!["send".into()],
            likely_tests: Vec::new(),
            impacted_symbols: Vec::new(),
            likely_tests_truncated: false,
            impacted_symbols_truncated: false,
            traversal_ceiling_reached: false,
            limit_at_maximum: false,
            likely_tests_found: 0,
            impacted_symbols_found: 0,
            stopped_at_depth: None,
            callers_report: Some(CallersReportSites {
                sites: 390,
                unresolved: 154,
                excluded: 236,
                ..CallersReportSites::default()
            }),
            open_ends: Vec::new(),
            tests_past_depth: None,
            tests_past_depth_cut: false,
            past_depth_tests: Vec::new(),
            report_symbols_unchecked: None,
            report_symbols_failed: None,
            report_sites_unchecked: None,
            coverage_gap: None,
        };
        let formatted = format_blast_radius(&unresolved_only);
        assert!(!formatted.contains("No downstream callers"), "{formatted}");
        assert!(formatted.contains(
            "### Downstream Impact (0 returned)\nThe index could not tie 154 sites of the name to this definition; find_references lists them.\n"
        ), "{formatted}");

        let excluded_only = BlastRadiusResult {
            callers_report: Some(CallersReportSites {
                sites: 2,
                excluded: 2,
                ..CallersReportSites::default()
            }),
            ..unresolved_only.clone()
        };
        let formatted = format_blast_radius(&excluded_only);
        assert!(!formatted.contains("No downstream callers"), "{formatted}");
        assert!(
            formatted
                .contains("find_references ties all 2 sites of the name to other definitions.\n"),
            "{formatted}"
        );

        let test_callers_only = BlastRadiusResult {
            callers_report: Some(CallersReportSites {
                sites: 3,
                likely: 2,
                unresolved: 1,
                ..CallersReportSites::default()
            }),
            ..unresolved_only.clone()
        };
        let formatted = format_blast_radius(&test_callers_only);
        assert!(formatted.contains(
            "Every caller that find_references ties to this definition is a test.\nThe index could not tie 1 more site of the name to this definition; find_references lists it.\n"
        ), "{formatted}");

        let no_sites = BlastRadiusResult {
            callers_report: Some(CallersReportSites::default()),
            ..unresolved_only
        };
        assert!(
            format_blast_radius(&no_sites).contains("No downstream callers found within depth.\n")
        );
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

    fn leaf(id: &str, parent: &str, signature: &str, line: usize) -> Symbol {
        skeleton_row(
            id,
            Some(parent),
            "field",
            signature,
            signature,
            (line, line),
            None,
        )
    }

    #[test]
    fn a_cut_keeps_a_longer_prefix_when_admitting_the_last_row_of_a_group_drops_its_fold() {
        let r1 = skeleton_row("r1", None, "module", "r1", "mod r1", (1, 10), None);
        let r2 = skeleton_row("r2", None, "module", "r2", "mod r2", (11, 200), None);
        let mut rows = vec![
            leaf("a", "r1", "a", 2),
            leaf("b", "r1", "b", 3),
            leaf("c", "r1", "c", 4),
        ];
        rows.extend((0..20).map(|i| {
            leaf(
                &format!("f{i}"),
                "r2",
                &format!("field_number_{i}: SomeLongType"),
                12 + i,
            )
        }));
        let mut syms = vec![r1, r2];
        syms.extend(rows);
        let children_map = children_in_file(&syms);
        let roots: Vec<&Symbol> = children_map[&None].clone();
        let expected = "mod r1 {\n    a; // L2-2\n    b; // L3-3\n    c; // L4-4\n} // L1-10\n\n\
                        mod r2 { /* 20 rows hidden: L11-L200 */ }\n";

        let (out, cut) = render_rows_within(&roots, None, &children_map, 0, expected.len());

        assert!(cut);
        assert_eq!(out, expected);
    }

    #[test]
    fn a_collapsed_row_spans_the_lines_of_members_outside_its_own_declaration() {
        let syms = vec![
            skeleton_row("s", None, "struct", "S", "pub struct S", (1, 3), None),
            leaf("x", "s", "pub x: u8", 2),
            skeleton_row(
                "m",
                Some("s"),
                "method",
                "m",
                "pub fn m(&self)",
                (20, 30),
                Some((21, 30)),
            ),
        ];
        let children_map = children_in_file(&syms);

        let (out, cut) = render_rows_within(&[&syms[0]], None, &children_map, 0, 50);

        assert!(cut);
        assert_eq!(out, "pub struct S { /* 2 rows hidden: L1-L30 */ }\n");
    }

    #[test]
    fn the_summary_of_a_file_with_no_shown_row_cuts_a_long_name() {
        let name = "x".repeat(33_000);
        let syms = vec![script_row(
            "javascript",
            "variable",
            &name,
            "const x = 1",
            1,
        )];

        let out = format_file_skeleton("gen.js", &syms, Some(1), 0);

        assert!(out.len() < 1_000, "{}", out.len());
        assert!(out.contains(&format!("`{}…` L1", "x".repeat(120))), "{out}");
    }

    #[test]
    fn a_skeleton_over_the_budget_collapses_the_deepest_rows_and_folds_the_rest() {
        let mut syms = vec![skeleton_row(
            "root",
            None,
            "module",
            "shapes",
            "mod shapes",
            (1, 9002),
            None,
        )];
        for i in 0..3000 {
            let start = 2 + i * 3;
            syms.push(skeleton_row(
                &format!("s{i}"),
                Some("root"),
                "struct",
                &format!("Shape{i}"),
                &format!("pub struct Shape{i}"),
                (start, start + 2),
                None,
            ));
            syms.push(skeleton_row(
                &format!("f{i}"),
                Some(&format!("s{i}")),
                "field",
                "sides",
                "pub sides: u8",
                (start + 1, start + 1),
                None,
            ));
        }

        let out = format_file_skeleton("src/shapes.rs", &syms, Some(9002), 0);

        assert!(out.len() <= OUTPUT_BUDGET_CHARS, "{}", out.len());
        assert!(out.contains(SKELETON_CUT_NOTE));
        assert!(
            out.contains("mod shapes {\n    pub struct Shape0 { /* 1 row hidden: L2-L4 */ }\n")
        );
        assert!(!out.contains("pub sides: u8"));
        let fold = out
            .lines()
            .find(|line| line.contains(" more rows: L"))
            .unwrap();
        assert!(
            fold.starts_with("    /* ") && fold.ends_with("-L9001 */"),
            "{fold}"
        );
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

    fn script_row(language: &str, kind: &str, name: &str, signature: &str, line: usize) -> Symbol {
        Symbol {
            language: language.into(),
            ..skeleton_row(name, None, kind, name, signature, (line, line), None)
        }
    }

    #[test]
    fn a_skeleton_with_only_top_level_variables_and_imports_names_them() {
        let syms = vec![
            script_row(
                "javascript",
                "import",
                "express",
                "var express = require('../../')",
                7,
            ),
            script_row(
                "javascript",
                "variable",
                "app",
                "var app = module.exports = express()",
                8,
            ),
            script_row(
                "javascript",
                "import",
                "logger",
                "var logger = require('morgan')",
                9,
            ),
            script_row(
                "javascript",
                "import",
                "cookieParser",
                "var cookieParser = require('cookie-parser')",
                10,
            ),
            script_row("javascript", "variable", "minute", "var minute = 60000", 40),
        ];

        assert_eq!(
            format_file_skeleton("examples/cookies/index.js", &syms, Some(53), 0),
            "// File: examples/cookies/index.js (Lines 1-53)\n\
             \n\
             // Nothing to outline. Top-level rows: 3 imports (`express` L7, `logger` L9, `cookieParser` L10); 2 variables (`app` L8, `minute` L40).\n\
             // get_symbol_body with one of these names and this file_path shows its source; read the file for the rest.\n"
        );
    }

    #[test]
    fn a_python_skeleton_with_only_imports_counts_them_and_names_the_first_three() {
        let syms: Vec<Symbol> = ["json", "abort", "redirect", "Flask", "Blueprint"]
            .iter()
            .enumerate()
            .map(|(index, name)| {
                script_row(
                    "python",
                    "import",
                    name,
                    &format!("from .app import {name} as {name}"),
                    index + 1,
                )
            })
            .collect();

        assert_eq!(
            format_file_skeleton("src/flask/__init__.py", &syms, Some(39), 0),
            "// File: src/flask/__init__.py (Lines 1-39)\n\
             \n\
             // Nothing to outline. Top-level rows: 5 imports (`json` L1, `abort` L2, `redirect` L3, +2 more).\n\
             // get_symbol_body with one of these names and this file_path shows its source; read the file for the rest.\n"
        );
    }

    #[test]
    fn a_skeleton_that_shows_a_row_adds_no_summary_of_hidden_rows() {
        let syms = vec![
            script_row(
                "javascript",
                "import",
                "express",
                "var express = require('express')",
                1,
            ),
            script_row("javascript", "function", "start", "function start()", 3),
        ];

        let out = format_file_skeleton("index.js", &syms, Some(5), 0);

        assert!(out.contains("function start()"), "{out}");
        assert!(!out.contains("Nothing to outline"), "{out}");
    }

    #[test]
    fn a_skeleton_of_a_file_without_symbols_says_so() {
        assert_eq!(
            format_file_skeleton("notes.js", &[], Some(4), 0),
            "// File: notes.js (Lines 1-4)\n\n// No symbols are indexed for this file.\n"
        );
    }

    fn labeled_site(
        label: SiteLabel,
        path: &str,
        line: usize,
        caller: &str,
        basis: &str,
    ) -> LabeledSite {
        LabeledSite {
            site: ReferenceSite {
                from_symbol_name: caller.into(),
                from_symbol_id: "c1".into(),
                to_symbol_name: "target".into(),
                kind: "calls".into(),
                path: path.into(),
                start_line: Some(line),
                start_column: Some(4),
                occurrences: None,
                target: None,
                enclosing_symbol_name: None,
            },
            label,
            basis: basis.into(),
            end_line: None,
            arg_count: None,
        }
    }

    fn report_of(sites: Vec<LabeledSite>) -> ReferenceReport {
        let count = |label| sites.iter().filter(|site| site.label == label).count();
        ReferenceReport {
            target: ReferenceTarget {
                symbol_id: "t1".into(),
                name: "from_mapping".into(),
                display_name: "Config.from_mapping".into(),
                kind: "method".into(),
                path: "src/flask/config.py".into(),
                line: 304,
                definitions_with_name: 1,
            },
            counts: ReferenceCounts {
                sites: sites.len(),
                proven: count(SiteLabel::Proven),
                likely: count(SiteLabel::Likely),
                unresolved: count(SiteLabel::Unresolved),
                excluded: count(SiteLabel::Excluded),
                text_lines: sites.len(),
                site_lines: sites.len(),
                ..ReferenceCounts::default()
            },
            sites,
            text_only: Vec::new(),
            text_scan: TextScanSummary {
                files_scanned: 12,
                files_total: 12,
                complete: true,
                elapsed_ms: 40,
                skipped: None,
            },
            cannot_see: Vec::new(),
            imports: Vec::new(),
            overrides: Vec::new(),
            overridden: Vec::new(),
            constructions: Vec::new(),
            variant_uses: Vec::new(),
            registrations: Vec::new(),
            tests: Vec::new(),
            coverage_gap: None,
        }
    }

    #[test]
    fn the_report_header_has_three_lines_with_plural_counts() {
        let mut report = report_of(vec![
            labeled_site(
                SiteLabel::Proven,
                "src/flask/config.py",
                302,
                "Config.from_file",
                "same file",
            ),
            labeled_site(
                SiteLabel::Likely,
                "tests/test_config.py",
                112,
                "test_config",
                "name only",
            ),
        ]);
        report.counts.text_lines = 17;
        report.counts.site_lines = 2;
        report.counts.definition_lines = 2;
        report.counts.import_lines = 0;
        report.counts.text_only_lines = 13;

        let text = format_reference_report(&report, 20);

        assert_eq!(
            text.lines().take(3).collect::<Vec<_>>(),
            [
                "References to method `Config.from_mapping` (src/flask/config.py:304)",
                "Sites: 2 · proven 1 · likely 1 · unresolved 0 · excluded 0",
                "Text: `from_mapping` is on 17 lines: 2 sites, 2 definitions, 0 imports, 13 text only.",
            ]
        );
    }

    #[test]
    fn the_report_header_uses_singular_forms_for_a_count_of_one() {
        let mut report = report_of(vec![labeled_site(
            SiteLabel::Proven,
            "a.py",
            3,
            "caller",
            "same file",
        )]);
        report.counts.text_lines = 1;
        report.counts.site_lines = 1;
        report.counts.definition_lines = 1;
        report.counts.import_lines = 1;
        report.counts.text_only_lines = 0;

        let text = format_reference_report(&report, 20);

        assert_eq!(
            text.lines().nth(2),
            Some("Text: `from_mapping` is on 1 line: 1 site, 1 definition, 1 import, 0 text only.")
        );
    }

    #[test]
    fn the_report_lists_each_group_in_label_order_with_its_basis() {
        let report = report_of(vec![
            labeled_site(SiteLabel::Proven, "a.py", 2, "first", "same file"),
            labeled_site(SiteLabel::Likely, "l.py", 6, "second", "name only"),
            labeled_site(SiteLabel::Unresolved, "u.py", 4, "third", "3 definitions"),
            labeled_site(SiteLabel::Excluded, "x.py", 9, "other", "other language"),
        ]);

        let text = format_reference_report(&report, 20);

        let body = text.split_once("\n\n").unwrap().1;
        assert_eq!(
            body,
            "Proven (1):\n\
             - `first` [a.py:2] (kind: calls; same file)\n\
             Likely (1):\n\
             - `second` [l.py:6] (kind: calls; name only)\n\
             Unresolved (1):\n\
             - `third` [u.py:4] (kind: calls; 3 definitions)\n\
             Excluded (1):\n\
             - `other` [x.py:9] (kind: calls; other language)\n"
        );
    }

    #[test]
    fn a_group_with_no_sites_is_left_out() {
        let report = report_of(vec![labeled_site(
            SiteLabel::Likely,
            "l.py",
            6,
            "second",
            "name only",
        )]);

        let text = format_reference_report(&report, 20);

        assert!(!text.contains("Proven ("), "{text}");
        assert!(!text.contains("Excluded ("), "{text}");
        assert!(text.contains("Likely (1):\n"), "{text}");
    }

    #[test]
    fn a_report_without_sites_has_only_the_header() {
        let report = report_of(Vec::new());

        let text = format_reference_report(&report, 20);

        assert_eq!(
            text,
            "References to method `Config.from_mapping` (src/flask/config.py:304)\n\
             Sites: 0 · proven 0 · likely 0 · unresolved 0 · excluded 0\n\
             Text: `from_mapping` is on 0 lines: 0 sites, 0 definitions, 0 imports, 0 text only.\n"
        );
    }

    #[test]
    fn caller_names_follow_the_lambda_and_no_enclosing_symbol_rules() {
        let mut lambda = labeled_site(SiteLabel::Proven, "a.py", 1, "lambda_7", "same file");
        lambda.site.enclosing_symbol_name = Some("outer".into());
        let callerless = labeled_site(SiteLabel::Proven, "a.py", 2, "", "same file");
        let report = report_of(vec![lambda, callerless]);

        let text = format_reference_report(&report, 20);

        assert!(
            text.contains("- `<lambda> in outer` [a.py:1] (kind: calls; same file)\n"),
            "{text}"
        );
        assert!(
            text.contains("- (no enclosing symbol) [a.py:2] (kind: calls; same file)\n"),
            "{text}"
        );
    }

    #[test]
    fn each_label_lists_up_to_the_limit_and_folds_the_rest_with_per_file_counts() {
        let mut sites = vec![labeled_site(
            SiteLabel::Proven,
            "a.py",
            1,
            "p1",
            "same file",
        )];
        for (index, path) in ["b.py", "b.py", "b.py", "c.py", "c.py", "d.py"]
            .iter()
            .enumerate()
        {
            sites.push(labeled_site(
                SiteLabel::Likely,
                path,
                10 + index,
                "caller",
                "name only",
            ));
        }
        sites.push(labeled_site(
            SiteLabel::Excluded,
            "z.py",
            1,
            "other",
            "other language",
        ));
        let report = report_of(sites);

        let text = format_reference_report(&report, 3);

        let body = text.split_once("\n\n").unwrap().1;
        assert_eq!(
            body,
            "Proven (1):\n\
             - `p1` [a.py:1] (kind: calls; same file)\n\
             Likely (6):\n\
             - `caller` [b.py:10] (kind: calls; name only)\n\
             - `caller` [b.py:11] (kind: calls; name only)\n\
             - `caller` [b.py:12] (kind: calls; name only)\n\
             … 3 more likely sites in 2 files: c.py (2), d.py (1)\n\
             Excluded (1):\n\
             - `other` [z.py:1] (kind: calls; other language)\n"
        );
    }

    #[test]
    fn a_fold_line_names_at_most_five_files() {
        let sites = ["a", "b", "c", "d", "e", "f", "g"]
            .iter()
            .map(|stem| {
                labeled_site(
                    SiteLabel::Likely,
                    &format!("{stem}.py"),
                    1,
                    "caller",
                    "name only",
                )
            })
            .collect();
        let report = report_of(sites);

        let text = format_reference_report(&report, 1);

        assert!(
            text.contains(
                "… 6 more likely sites in 6 files: b.py (1), c.py (1), d.py (1), e.py (1), f.py (1), …\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn text_only_lines_list_the_first_limit_lines_per_file_and_fold_the_rest() {
        let mut report = report_of(Vec::new());
        report.text_only = (0..12)
            .map(|index| TextOnlyLines {
                path: format!("docs/f{index:02}.rst"),
                lines: if index == 0 {
                    (1..=15).collect()
                } else {
                    vec![index + 1, index + 2]
                },
            })
            .collect();
        report.counts.text_only_lines = report.text_only.iter().map(|f| f.lines.len()).sum();

        let text = format_reference_report(&report, 20);

        let section = text.split_once("Text only (").unwrap().1;
        let mut lines = section.lines();
        assert_eq!(
            lines.next(),
            Some("37): docs, comments, strings, or code the index did not record")
        );
        assert_eq!(
            lines.next(),
            Some("  docs/f00.rst: 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15")
        );
        assert_eq!(lines.next(), Some("  docs/f01.rst: 2, 3"));
        assert_eq!(lines.next(), Some("  docs/f02.rst: 3, 4"));
        assert_eq!(lines.next(), Some("  docs/f03.rst: 4"));
        assert_eq!(
            lines.next(),
            Some(
                "… 17 more lines in 9 files: docs/f04.rst (2), docs/f05.rst (2), docs/f06.rst (2), docs/f07.rst (2), docs/f08.rst (2), …"
            )
        );
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn a_text_scan_cut_by_its_budget_adds_the_stopped_line() {
        let mut report = report_of(Vec::new());
        report.text_scan = TextScanSummary {
            files_scanned: 410,
            files_total: 17334,
            complete: false,
            elapsed_ms: 3012,
            skipped: None,
        };

        let text = format_reference_report(&report, 20);

        assert_eq!(
            text.lines().nth(3),
            Some("Text scan stopped after 3 s: 410 of 17334 files read; counts cover those files.")
        );
    }

    #[test]
    fn a_registration_without_a_verb_or_template_or_route_pattern_prints_what_it_has() {
        let registration =
            |pattern_id: &str, verb: Option<&str>, template: Option<&str>| Registration {
                pattern_id: pattern_id.into(),
                verb: verb.map(Into::into),
                template: template.map(Into::into),
                path: "app.py".into(),
                line: 3,
                by_name: false,
            };
        let mut report = report_of(Vec::new());
        report.registrations = vec![
            registration("flask.route.v1", Some("GET"), Some("/a")),
            registration("axum.route.v1", None, Some("/b")),
            registration("hapi.route.v1", Some("PUT"), None),
            registration("rocket.mount.v1", Some("GET"), Some("/c")),
        ];

        let text = format_reference_report(&report, 20);

        assert!(
            text.contains(
                "Registrations (4):\n\
                 - route GET /a [app.py:3]\n\
                 - route /b [app.py:3]\n\
                 - route PUT hapi.route.v1 [app.py:3]\n\
                 - rocket.mount.v1 [app.py:3]\n"
            ),
            "{text}"
        );
    }

    #[test]
    fn a_constructor_report_replaces_the_text_line_with_the_skip_line() {
        let mut report = report_of(Vec::new());
        report.text_scan.skipped = Some("Flask".into());

        let text = format_reference_report(&report, 20);

        assert_eq!(
            text.lines().nth(2),
            Some(
                "Text: skipped for a constructor; its call sites spell the class name. Use find_references on `Flask`."
            )
        );
        assert!(!text.contains("is on"), "{text}");
        assert!(!text.contains("Text scan stopped"), "{text}");
    }

    #[test]
    fn the_report_ends_with_the_blind_spots_and_the_import_summary() {
        let mut report = report_of(vec![labeled_site(
            SiteLabel::Proven,
            "a.py",
            2,
            "first",
            "same file",
        )]);
        report.cannot_see = vec![
            "calls through getattr() or names built at runtime".into(),
            "calls inside macro bodies".into(),
        ];
        report.imports = vec![("b.py".into(), 1), ("c.py".into(), 4)];

        let text = format_reference_report(&report, 20);

        assert!(
            text.ends_with(
                "Not visible to this index: calls through getattr() or names built at runtime; calls inside macro bodies.\n\
                 Imported 2 times in 2 files: b.py:1, c.py:4 (lookup_symbol with kind=\"import\" lists them)\n"
            ),
            "{text}"
        );
    }
}
