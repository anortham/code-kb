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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuralFact {
    pub structural_fact_id: String,
    pub path: String,
    pub language: String,
    pub pattern_id: String,
    pub capture_name: String,
    pub node_kind: String,
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SymbolSearchResult {
    pub symbol: Symbol,
    pub score: f64,
    pub snippet: Option<String>,
}
