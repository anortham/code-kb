use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Symbol {
    pub symbol_id: String,
    pub file_id: String,
    pub path: String,
    pub language: String,
    pub name: String,
    pub kind: String,
    pub signature: Option<String>,
    pub doc_comment: Option<String>,
    pub visibility: Option<String>,
    pub parent_symbol_id: Option<String>,
    pub start_line: usize,
    pub start_column: usize,
    pub end_line: usize,
    pub end_column: usize,
    pub start_byte: usize,
    pub end_byte: usize,
    pub body_start_line: Option<usize>,
    pub body_start_column: Option<usize>,
    pub body_end_line: Option<usize>,
    pub body_end_column: Option<usize>,
    pub body_start_byte: Option<usize>,
    pub body_end_byte: Option<usize>,
    pub body_hash: Option<String>,
    pub semantic_group: Option<String>,
    pub is_test: bool,
    pub test_container: bool,
}

impl Symbol {
    /// Helper to compute hidden line count in implementation body if present.
    pub fn hidden_body_line_count(&self) -> Option<usize> {
        match (self.body_start_line, self.body_end_line) {
            (Some(start), Some(end)) if end >= start => Some(end - start + 1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FileFact {
    pub file_id: String,
    pub path: String,
    pub language: String,
    pub content_hash: String,
    pub content_bytes: i64,
    pub line_count: Option<i64>,
    pub indexed_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferenceSite {
    pub from_symbol_name: String,
    pub from_symbol_id: String,
    pub to_symbol_name: String,
    pub kind: String,
    pub path: String,
    pub start_line: Option<usize>,
    pub start_column: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occurrences: Option<usize>,
    /// For a callee: the definitions the call can reach, as `Owner.name` (path:line).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Text-only owner context for generated lambda names; omitted from JSON.
    #[serde(skip)]
    pub enclosing_symbol_name: Option<String>,
}

/// How well the index evidence ties a name site to the selected definition.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum SiteLabel {
    Proven,
    Likely,
    Unresolved,
    Excluded,
}

impl SiteLabel {
    pub const ALL: [SiteLabel; 4] = [
        SiteLabel::Proven,
        SiteLabel::Likely,
        SiteLabel::Unresolved,
        SiteLabel::Excluded,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            SiteLabel::Proven => "proven",
            SiteLabel::Likely => "likely",
            SiteLabel::Unresolved => "unresolved",
            SiteLabel::Excluded => "excluded",
        }
    }

    /// Position in the report order: proven, likely, unresolved, excluded.
    pub fn rank(self) -> usize {
        match self {
            SiteLabel::Proven => 0,
            SiteLabel::Likely => 1,
            SiteLabel::Unresolved => 2,
            SiteLabel::Excluded => 3,
        }
    }
}

/// One line of the project that holds the target's name, with the evidence that ties it to the
/// target. `end_line` is set only when the site spans more than its first line.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LabeledSite {
    #[serde(flatten)]
    pub site: ReferenceSite,
    pub label: SiteLabel,
    pub basis: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<usize>,
    /// The number of arguments the call passes, when julie recorded it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arg_count: Option<u32>,
}

/// Lines that hold the target's name as a whole word but have no site, definition, or import.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextOnlyLines {
    pub path: String,
    pub lines: Vec<usize>,
}

/// What the whole-word text scan covered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextScanSummary {
    pub files_scanned: usize,
    pub files_total: usize,
    pub complete: bool,
    pub elapsed_ms: u64,
    /// Set to the class name when the scan did not run because the target is a constructor,
    /// whose call sites spell the class name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ReferenceCounts {
    pub sites: usize,
    pub proven: usize,
    pub likely: usize,
    pub unresolved: usize,
    pub excluded: usize,
    pub text_lines: usize,
    pub site_lines: usize,
    pub definition_lines: usize,
    pub import_lines: usize,
    pub text_only_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferenceTarget {
    pub symbol_id: String,
    pub name: String,
    /// `Owner.name` for a member of a class or another code type, else `name`.
    pub display_name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub definitions_with_name: usize,
}

/// A method of the same name in another type of the target owner's inheritance chain: an
/// override of the target, or a method that the target overrides.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OverrideRow {
    pub symbol_id: String,
    pub owner: String,
    pub name: String,
    pub path: String,
    pub line: usize,
}

/// A place that builds a value of the target type: a call or `new`, or a Rust or Go struct literal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConstructionSite {
    pub path: String,
    pub line: usize,
}

/// A function that names some variants of the enum target, where a `match` may need a new arm.
/// `caller` and `caller_id` are empty for code outside any symbol; `line` is the first use.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VariantUse {
    pub caller: String,
    pub caller_id: String,
    pub path: String,
    pub line: usize,
    pub variants_used: usize,
    pub variants_total: usize,
}

/// A route or other framework fact that registers the target function as a handler. `template`
/// is the route path (with any prefix joined) when the fact has one; `by_name` is true when only
/// the name ties the fact to the target.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Registration {
    pub pattern_id: String,
    pub verb: Option<String>,
    pub template: Option<String>,
    pub path: String,
    pub line: usize,
    pub by_name: bool,
}

/// A test that reaches the target through a proven or likely site: a test function or method, or
/// the test that holds a helper which has the site. `command` runs that test when the markers of
/// the project name the runner.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallingTest {
    pub symbol_id: String,
    pub name: String,
    pub path: String,
    pub line: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// Only `likely` sites with the basis `name only` tie the test to the target.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub name_only: bool,
}

/// Every indexed site of one definition's name, labeled by evidence, with a whole-word text count
/// that accounts for each line of the name. `sites` is complete and sorted by label (proven,
/// likely, unresolved, excluded), then path, line, and column. The section arrays come after
/// labeling, never change a label, and are left out of JSON when empty.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReferenceReport {
    pub target: ReferenceTarget,
    pub counts: ReferenceCounts,
    pub sites: Vec<LabeledSite>,
    pub text_only: Vec<TextOnlyLines>,
    pub text_scan: TextScanSummary,
    pub cannot_see: Vec<String>,
    pub imports: Vec<(String, usize)>,
    /// Methods of subtypes that calls through the target can reach.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<OverrideRow>,
    /// Methods of ancestor types that the target overrides, nearest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overridden: Vec<OverrideRow>,
    /// Places that build a value of a class, struct, or record target, one row per line.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constructions: Vec<ConstructionSite>,
    /// Functions that name variants of an enum target, most variants first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variant_uses: Vec<VariantUse>,
    /// Route facts and other framework facts that register the target function as a handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub registrations: Vec<Registration>,
    /// Tests with a proven or likely site of the target, each with the command that runs it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tests: Vec<CallingTest>,
    /// The target's language records no reference sites, so sites inside its files are missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_gap: Option<CoverageGap>,
}

pub(crate) fn has_generated_lambda_name(name: &str) -> bool {
    name.strip_prefix("lambda_")
        .is_some_and(|line| !line.is_empty() && line.bytes().all(|byte| byte.is_ascii_digit()))
}

pub(crate) fn is_generated_lambda_name(
    name: &str,
    language: &str,
    kind: &str,
    signature: Option<&str>,
) -> bool {
    has_generated_lambda_name(name)
        && language == "python"
        && kind == "function"
        && signature.is_some_and(|signature| signature.starts_with("lambda"))
}

/// The prefix of the name that code-kb gives an anonymous class: `<anonymous Task in make>`.
pub(crate) const ANONYMOUS_CLASS_PREFIX: &str = "<anonymous";

/// Whether a symbol is an anonymous class: a `class` row that code-kb named
/// `<anonymous Task in make>` in place of julie's `anonymous_class_L<line>`.
pub(crate) fn is_anonymous_class(name: &str, kind: &str) -> bool {
    kind == "class" && name.starts_with(ANONYMOUS_CLASS_PREFIX)
}

/// The name of an anonymous class: the type it creates or extends, from the creation head that
/// julie writes as its signature (`new Task()`, `object : Task`, `new Task { … }`,
/// `class extends Base`, `class implements Task`, `Class.new(Base)`, `{ new ITask with … }`),
/// and the function, method, property, or variable that creates it.
pub(crate) fn anonymous_class_label(signature: &str, creator: Option<&str>) -> String {
    let created = created_type(signature.trim());
    match (created, creator) {
        (Some(created), Some(creator)) => format!("<anonymous {created} in {creator}>"),
        (Some(created), None) => format!("<anonymous {created}>"),
        (None, Some(creator)) => format!("<anonymous in {creator}>"),
        (None, None) => "<anonymous>".to_string(),
    }
}

fn created_type(head: &str) -> Option<&str> {
    let after_word = |text: &'_ str, word: &str| -> Option<usize> {
        text.match_indices(word)
            .find(|(at, _)| *at == 0 || text[..*at].ends_with(char::is_whitespace))
            .map(|(at, _)| at + word.len())
    };
    let class_expression = |text: &str| {
        text.strip_prefix("class")
            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric() || c == '_'))
    };
    let head = match head.strip_prefix("new ") {
        Some(rest) if class_expression(rest.trim_start()) => rest.trim_start(),
        _ => head,
    };
    let written = if let Some(rest) = head.strip_prefix("new ") {
        rest
    } else if let Some(rest) = head.strip_prefix("object") {
        rest.trim_start().strip_prefix(':')?
    } else if class_expression(head) {
        let at = after_word(head, "extends ").or_else(|| after_word(head, "implements "))?;
        &head[at..]
    } else if let Some((receiver, rest)) = head.split_once(".new").or(head.split_once(".define")) {
        match receiver.trim() {
            "Class" => rest.trim_start().strip_prefix('(')?,
            receiver => receiver,
        }
    } else {
        return None;
    };
    let written = written.trim_start().trim_start_matches('\\');
    let end = written
        .find(|c: char| !(c.is_alphanumeric() || matches!(c, '_' | '.' | ':' | '\\' | '$')))
        .unwrap_or(written.len());
    let segment = written[..end]
        .rsplit(['.', ':', '\\'])
        .next()
        .filter(|segment| segment.starts_with(|c: char| c.is_alphabetic() || c == '_'))?;
    Some(segment)
}

/// A route or configuration key that a `lookup_symbol` query names: `kind` is `route` or
/// `config`, `key` is the route as its source spells it with its verb, or the dotted key path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LookupFact {
    pub kind: String,
    pub key: String,
    pub path: String,
    pub line: usize,
    /// The function or controller action that serves a route, when the fact names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    pub pattern_id: String,
}

/// Languages whose julie capability table records an `exception` for reference identifiers or
/// pending relationships, so the index holds no reference sites inside their files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CoverageGap {
    pub languages: Vec<String>,
    /// The capabilities with the exception, of `identifiers` and `pending_relationships`.
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuralFact {
    pub structural_fact_id: String,
    pub path: String,
    pub language: String,
    pub pattern_id: String,
    pub capture_name: String,
    pub node_kind: String,
    pub key: Option<String>,
    pub metadata: Option<serde_json::Value>,
    pub containing_symbol_name: Option<String>,
    pub start_line: usize,
    pub end_line: usize,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LiteralFact {
    pub literal_id: String,
    pub path: String,
    pub literal_text: String,
    pub kind: String,
    pub carrier: Option<String>,
    pub start_line: usize,
    pub containing_symbol_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TypeFact {
    pub type_fact_id: String,
    pub symbol_id: String,
    pub language: String,
    pub resolved_type: String,
    pub generic_params: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextSlice {
    pub target_symbol: Symbol,
    pub target_body: String,
    pub callee_signatures: Vec<String>,
    pub related_types: Vec<String>,
    pub related_tests: Vec<Symbol>,
}

/// One `search` hit. `score` is the rerank score; `explain` is present only when the
/// caller asked for the breakdown.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolSearchResult {
    pub symbol: Symbol,
    pub score: f64,
    pub snippet: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub explain: Option<SearchExplain>,
}

/// Rerank breakdown for one `search` hit: which recall branches admitted the row, the name
/// tier with its match strength, every score part, the query terms with the field that
/// credited each and each term's rarity weight, and the timer over the whole candidate set.
/// `score` is `term_score + field_diversity_bonus + name_bonus + kind_prior + path_role + documentation + test_intent + nested`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchExplain {
    pub bm25: Option<f64>,
    pub branches: Vec<String>,
    pub name_tier: String,
    pub name_strength: u32,
    pub terms: Vec<(String, String, f64)>,
    pub term_score: f64,
    #[serde(default)]
    pub field_diversity_bonus: f64,
    pub name_bonus: f64,
    pub kind_prior: f64,
    pub path_role: f64,
    pub documentation: f64,
    pub test_intent: f64,
    #[serde(default)]
    pub nested: f64,
    pub word_weights: Vec<(String, f64)>,
    pub candidates: usize,
    pub rerank_us: u128,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BlastRadiusResult {
    pub seed_type: String,
    pub seeds: Vec<String>,
    pub likely_tests: Vec<TestTarget>,
    pub impacted_symbols: Vec<ImpactedSymbol>,
    pub likely_tests_truncated: bool,
    pub impacted_symbols_truncated: bool,
    #[serde(default)]
    pub traversal_ceiling_reached: bool,
    /// The request already used the largest `limit`, so a larger one cannot show more rows.
    #[serde(default)]
    pub limit_at_maximum: bool,
    /// Likely tests found before `limit` cut the list.
    #[serde(default)]
    pub likely_tests_found: usize,
    /// Impacted symbols found before `limit` cut the list.
    #[serde(default)]
    pub impacted_symbols_found: usize,
    /// The depth limit of a walk that has callers beyond it, so a deeper walk may find more.
    #[serde(default)]
    pub stopped_at_depth: Option<usize>,
    /// The site counts of the target's callers report, when its callers stand in for a walk that
    /// found no impacted symbol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub callers_report: Option<CallersReportSites>,
    /// Symbols the walk reached that no indexed code calls, so tests that reach the target
    /// through them are not listed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_ends: Vec<OpenEnd>,
    /// When the walk stopped at a depth below the largest, the tests that a walk at the largest
    /// depth adds to the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests_past_depth: Option<usize>,
    /// The walk at the largest depth stopped at the 200-row ceiling, so `tests_past_depth` is a
    /// lower bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tests_past_depth_cut: bool,
    /// The path and line of each test that `tests_past_depth` counts.
    #[serde(skip)]
    pub(crate) past_depth_tests: Vec<(String, usize)>,
    /// The symbols of a file target whose callers report did not run within the time limit, so
    /// the tests that only their reports tie to the target are missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_symbols_unchecked: Option<usize>,
    /// The symbols whose callers report failed with an error, so the callers and tests that only
    /// their reports tie to the target are missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_symbols_failed: Option<usize>,
    /// The sites that the callers reports did not check within their time limit. Such a site is
    /// `likely` or `unresolved` without the checks, so a caller or test it would tie may be
    /// missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_sites_unchecked: Option<usize>,
    /// Languages of the target that record no reference sites, so callers inside their files
    /// are missing from the walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coverage_gap: Option<CoverageGap>,
}

/// A symbol where the walk stopped because the index shows no caller of it, such as a method
/// that the runtime or a framework calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenEnd {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub depth: usize,
    /// Text-only owner name, as `App` for `App.__call__`; omitted from JSON.
    #[serde(skip)]
    pub owner: Option<String>,
}

/// How the callers report labeled the sites of the target's name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CallersReportSites {
    pub sites: usize,
    pub proven: usize,
    pub likely: usize,
    pub unresolved: usize,
    pub excluded: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TestTarget {
    pub name: String,
    pub path: String,
    pub line: usize,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ImpactedSymbol {
    pub name: String,
    pub kind: String,
    pub path: String,
    pub line: usize,
    pub depth: usize,
    /// Text-only owner context for generated lambda names; omitted from JSON.
    #[serde(skip)]
    pub enclosing_symbol_name: Option<String>,
    /// Why a row that the callers report found, not the walk, is impacted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::anonymous_class_label;

    #[test]
    fn an_anonymous_class_is_named_by_the_type_it_creates_and_its_creator() {
        let cases = [
            ("new Task()", "<anonymous Task in make>"),
            (
                "new java.util.Comparator<String>()",
                "<anonymous Comparator in make>",
            ),
            (
                "object : MouseAdapter(), Closeable",
                "<anonymous MouseAdapter in make>",
            ),
            ("object", "<anonymous in make>"),
            ("new Logging with Closeable", "<anonymous Logging in make>"),
            (
                "class extends Base implements Task",
                "<anonymous Base in make>",
            ),
            ("class implements Task", "<anonymous Task in make>"),
            (
                "new class implements \\Psr\\Log\\LoggerInterface",
                "<anonymous LoggerInterface in make>",
            ),
            ("new class($a, $b)", "<anonymous in make>"),
            ("new classic.Task()", "<anonymous Task in make>"),
            ("class", "<anonymous in make>"),
            ("class($a) extends \\App\\Base", "<anonymous Base in make>"),
            ("Class.new(Api::Base)", "<anonymous Base in make>"),
            ("Class.new", "<anonymous in make>"),
            ("Struct.new(:a, :b)", "<anonymous Struct in make>"),
            ("Data.define(:a)", "<anonymous Data in make>"),
            ("new ITask", "<anonymous ITask in make>"),
            ("new Base(1)", "<anonymous Base in make>"),
        ];
        for (signature, label) in cases {
            assert_eq!(
                anonymous_class_label(signature, Some("make")),
                label,
                "{signature}"
            );
        }
        assert_eq!(
            anonymous_class_label("new Task()", None),
            "<anonymous Task>"
        );
        assert_eq!(anonymous_class_label("", None), "<anonymous>");
    }
}
