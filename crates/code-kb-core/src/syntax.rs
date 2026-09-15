use serde::Deserialize;
use std::io::Write;
use std::process::{Command, Stdio};
use thiserror::Error;

use crate::sync::find_julie_extract_binary;

#[derive(Debug, Error, PartialEq)]
pub enum SyntaxError {
    #[error("Syntax error in {0}: {1}")]
    ParseError(String, String),
    #[error("Syntax check could not run: {0}")]
    CheckFailed(String),
}

#[derive(Deserialize)]
struct CheckReport {
    status: String,
    #[serde(default)]
    errors: Vec<CheckDiagnostic>,
}

#[derive(Deserialize)]
struct CheckDiagnostic {
    message: String,
}

/// Validates `content` as the full text of `file_path` through `julie-extract check` before it is written to disk.
/// Returns `Ok(true)` when the extractor parsed it cleanly, `Ok(false)` when the extractor has no grammar for the
/// path, and `Err(SyntaxError::ParseError)` when the parse reported syntax errors.
pub fn validate_syntax(file_path: &str, content: &str) -> Result<bool, SyntaxError> {
    let bin = find_julie_extract_binary()
        .ok_or_else(|| SyntaxError::CheckFailed("julie-extract binary not found".to_string()))?;
    let mut child = Command::new(bin)
        .args(["check", "--path", file_path, "--json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| SyntaxError::CheckFailed(e.to_string()))?;
    let written = child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(content.as_bytes());
    let output = child
        .wait_with_output()
        .map_err(|e| SyntaxError::CheckFailed(e.to_string()))?;
    written.map_err(|e| SyntaxError::CheckFailed(format!("could not send content: {e}")))?;
    let report: CheckReport = serde_json::from_slice(&output.stdout)
        .map_err(|e| SyntaxError::CheckFailed(format!("unreadable check report: {e}")))?;

    match report.status.as_str() {
        "ok" => Ok(true),
        "unsupported" => Ok(false),
        _ => {
            let first = report
                .errors
                .first()
                .map(|e| e.message.clone())
                .unwrap_or_else(|| "syntax error".to_string());
            let more = report.errors.len().saturating_sub(1);
            let detail = if more > 0 {
                format!("{first} (+{more} more)")
            } else {
                first
            };
            Err(SyntaxError::ParseError(file_path.to_string(), detail))
        }
    }
}
