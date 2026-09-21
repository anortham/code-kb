use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CallToolResult {
    pub content: Vec<TextContent>,
    #[serde(
        rename = "isError",
        skip_serializing_if = "std::ops::Not::not",
        default
    )]
    pub is_error: bool,
    #[serde(skip)]
    pub logical_result_count: Option<usize>,
    #[serde(skip)]
    pub reconcile_ms: Option<u64>,
    #[serde(skip)]
    pub query_ms: Option<u64>,
    /// Workspace-relative files this answer points into, used as the tokens-saved baseline.
    #[serde(skip)]
    pub baseline_paths: Vec<String>,
}

impl CallToolResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![TextContent {
                content_type: "text".to_string(),
                text: text.into(),
            }],
            is_error: false,
            logical_result_count: None,
            reconcile_ms: None,
            query_ms: None,
            baseline_paths: Vec::new(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            content: vec![TextContent {
                content_type: "text".to_string(),
                text: message.into(),
            }],
            is_error: true,
            logical_result_count: None,
            reconcile_ms: None,
            query_ms: None,
            baseline_paths: Vec::new(),
        }
    }

    pub fn with_logical_result_count(mut self, count: usize) -> Self {
        self.logical_result_count = Some(count);
        self
    }

    pub fn with_baseline_paths(mut self, paths: Vec<String>) -> Self {
        self.baseline_paths = paths;
        self
    }
}
