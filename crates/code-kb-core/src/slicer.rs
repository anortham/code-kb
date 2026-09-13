use std::fs;
use std::path::Path;
use thiserror::Error;

use crate::models::Symbol;

#[derive(Debug, Error)]
pub enum SliceError {
    #[error("Failed to read file '{0}': {1}")]
    Io(String, #[source] std::io::Error),
    #[error("Invalid slice range: start {0} > end {1}")]
    InvalidRange(usize, usize),
    #[error("File '{0}' is empty")]
    EmptyFile(String),
}

/// Snap byte offset to the nearest UTF-8 character boundary <= index.
fn snap_to_char_boundary_floor(bytes: &[u8], mut idx: usize) -> usize {
    if idx >= bytes.len() {
        return bytes.len();
    }
    while idx > 0 && (bytes[idx] & 0xC0) == 0x80 {
        idx -= 1;
    }
    idx
}

/// Snap byte offset to the nearest UTF-8 character boundary >= index.
fn snap_to_char_boundary_ceil(bytes: &[u8], mut idx: usize) -> usize {
    if idx >= bytes.len() {
        return bytes.len();
    }
    while idx < bytes.len() && (bytes[idx] & 0xC0) == 0x80 {
        idx += 1;
    }
    idx
}

/// Slices exact byte content from an in-memory buffer with safe UTF-8 snapping.
pub fn slice_bytes_safe(content: &[u8], start_byte: usize, end_byte: usize) -> Result<&str, SliceError> {
    if start_byte > end_byte {
        return Err(SliceError::InvalidRange(start_byte, end_byte));
    }

    let start = snap_to_char_boundary_floor(content, start_byte.min(content.len()));
    let end = snap_to_char_boundary_ceil(content, end_byte.min(content.len()));

    std::str::from_utf8(&content[start..end]).map_err(|_| SliceError::InvalidRange(start, end))
}

/// Slice the entire symbol declaration (signature + body) from file on disk.
pub fn slice_symbol(file_path: &Path, symbol: &Symbol) -> Result<String, SliceError> {
    let bytes = fs::read(file_path)
        .map_err(|e| SliceError::Io(file_path.display().to_string(), e))?;

    if bytes.is_empty() {
        return Err(SliceError::EmptyFile(file_path.display().to_string()));
    }

    // Try byte span first
    if symbol.end_byte <= bytes.len()
        && symbol.start_byte <= symbol.end_byte
        && symbol.end_byte > 0
        && let Ok(slice) = slice_bytes_safe(&bytes, symbol.start_byte, symbol.end_byte)
    {
        return Ok(slice.to_string());
    }

    // Fallback to line slicing
    slice_lines(&bytes, symbol.start_line, symbol.end_line)
}

/// Slice only the symbol's implementation body from file on disk.
pub fn slice_symbol_body(file_path: &Path, symbol: &Symbol) -> Result<String, SliceError> {
    let bytes = fs::read(file_path)
        .map_err(|e| SliceError::Io(file_path.display().to_string(), e))?;

    if bytes.is_empty() {
        return Err(SliceError::EmptyFile(file_path.display().to_string()));
    }

    // Check if body byte spans are present
    if let (Some(body_start), Some(body_end)) = (symbol.body_start_byte, symbol.body_end_byte)
        && body_end <= bytes.len()
        && body_start <= body_end
        && let Ok(slice) = slice_bytes_safe(&bytes, body_start, body_end)
    {
        return Ok(slice.to_string());
    }

    // Fallback to body line span
    if let (Some(b_start), Some(b_end)) = (symbol.body_start_line, symbol.body_end_line) {
        return slice_lines(&bytes, b_start, b_end);
    }

    // If no body defined, return entire symbol
    slice_symbol(file_path, symbol)
}

/// Fallback 1-based line slicer
pub fn slice_lines(bytes: &[u8], start_line: usize, end_line: usize) -> Result<String, SliceError> {
    if start_line > end_line {
        return Err(SliceError::InvalidRange(start_line, end_line));
    }

    let text = String::from_utf8_lossy(bytes);
    let mut selected = Vec::new();

    for (idx, line) in text.lines().enumerate() {
        let line_num = idx + 1;
        if line_num >= start_line && line_num <= end_line {
            selected.push(line);
        }
        if line_num > end_line {
            break;
        }
    }

    Ok(selected.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slice_bytes_safe() {
        let sample = "fn hello() {\n    println!(\"hi\");\n}\n";
        let sliced = slice_bytes_safe(sample.as_bytes(), 0, sample.len()).unwrap();
        assert_eq!(sliced, sample);
    }

    #[test]
    fn test_slice_lines() {
        let sample = "line 1\nline 2\nline 3\nline 4\n";
        let sliced = slice_lines(sample.as_bytes(), 2, 3).unwrap();
        assert_eq!(sliced, "line 2\nline 3");
    }
}
