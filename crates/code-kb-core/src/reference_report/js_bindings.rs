//! Finds where a JavaScript or TypeScript file binds a name as a callback parameter, a loop
//! variable, or a catch parameter. julie records no symbol for these, so only the source text
//! shows that they hide an import.

use std::rc::Rc;

use super::js_modules::BindingSites;
use super::{Accounting, NameRow, QueryError, is_word_char, language_family};

/// Where a file binds a name, and where the block body that holds the binding ends, as
/// `(line, column)`.
#[derive(Clone, Copy)]
pub(super) struct Binding {
    at: (usize, usize),
    body_end: Option<(usize, usize)>,
}

/// The longest parameter list or loop head read, in bytes.
const MAX_LIST: usize = 400;
/// The longest return type read before the `=>` of a typed arrow function, in bytes.
const MAX_RETURN_TYPE: usize = 200;
/// Keywords after which a `(` can open the parameters of an arrow function, not the arguments of
/// a call.
const ARROW_KEYWORDS: &[&str] = &["return", "throw", "yield", "else", "do"];

struct Level<'a> {
    open: u8,
    /// The next name is a binding.
    want: bool,
    /// Inside a type annotation or default value, up to the next `,`.
    skip: bool,
    /// Inside brackets that hold no binding: a type argument, a default value, a call.
    inert: bool,
    /// The name before a `:`, `,`, `=`, or `}` of an object pattern, which is a key or a binding.
    pending: Option<&'a str>,
}

impl Accounting<'_> {
    /// The line above the row, inside its caller, that binds the first name the row writes (the
    /// name itself, or `z` of `z.core.f()`) as a callback parameter, loop variable, or catch
    /// parameter.
    pub(super) fn js_binding_line(
        &self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<usize>, QueryError> {
        if language_family(&row.language) != "js" || row.hidden_receiver || row.kind == "type_usage"
        {
            return Ok(None);
        }
        let head = match &row.chain {
            Some(chain) => chain.first().map_or(name, String::as_str),
            None if !row.has_qualifier() => name,
            None => return Ok(None),
        };
        if self.js_row_is_binding(row, head) {
            return Ok(Some(row.site_line));
        }
        self.nearest_binding(row, head, |_| true)
    }

    /// The line above a bare row, inside its caller, that binds the row's name as a callback
    /// parameter, loop variable, or catch parameter whose `{ … }` body holds the row, so the row
    /// reads that binding. A row on the bound name itself is that binding (julie 3.11.0 writes a
    /// catch parameter as a `write` identifier), so its own line is returned.
    pub(super) fn js_enclosing_binding_line(
        &self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<usize>, QueryError> {
        if language_family(&row.language) != "js" || row.hidden_receiver || !row.is_bare_use() {
            return Ok(None);
        }
        if self.js_row_is_binding(row, name) {
            return Ok(Some(row.site_line));
        }
        let site = (row.site_line, row.site_column.unwrap_or(usize::MAX));
        self.nearest_binding(row, name, |binding| {
            binding.body_end.is_some_and(|end| site < end)
        })
    }

    /// Whether the row sits on the bound name itself, so it is that binding.
    fn js_row_is_binding(&self, row: &NameRow, name: &str) -> bool {
        let Some(column) = row.site_column else {
            return false;
        };
        self.js_binding_sites(&row.path, name)
            .iter()
            .any(|binding| binding.at == (row.site_line, column))
    }

    fn nearest_binding(
        &self,
        row: &NameRow,
        name: &str,
        holds: impl Fn(&Binding) -> bool,
    ) -> Result<Option<usize>, QueryError> {
        let bindings = self.js_binding_sites(&row.path, name);
        if bindings.is_empty() {
            return Ok(None);
        }
        let caller_start = match row.caller_id.as_deref() {
            Some(id) => self
                .definition_by_id(id)?
                .map_or(1, |caller| caller.start_line),
            None => 1,
        };
        let site = (row.site_line, row.site_column.unwrap_or(usize::MAX));
        Ok(bindings
            .iter()
            .rev()
            .find(|binding| (caller_start, 0) <= binding.at && binding.at < site && holds(binding))
            .map(|binding| binding.at.0))
    }

    /// The `(line, column)` of every place `path` binds `name`, in source order.
    fn js_binding_sites(&self, path: &str, name: &str) -> BindingSites {
        let key = (path.to_string(), name.to_string());
        if let Some(sites) = self.js.bindings.borrow().get(&key) {
            return sites.clone();
        }
        let sites = Rc::new(
            std::fs::read_to_string(self.root.join(path))
                .map(|text| binding_sites(&text, name))
                .unwrap_or_default(),
        );
        self.js.bindings.borrow_mut().insert(key, sites.clone());
        sites
    }
}

fn binding_sites(text: &str, name: &str) -> Vec<Binding> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let position = |at: usize| {
        let line = line_starts.partition_point(|&start| start <= at);
        (line, at - line_starts[line - 1])
    };
    text.match_indices(name)
        .filter_map(|(at, _)| {
            let after = binding_end(text, at, name)?;
            Some(Binding {
                at: position(at),
                body_end: block_body_end(text, after).map(position),
            })
        })
        .collect()
}

/// The index after a binding of the word `name` at `at`: after `name` in `name =>`, or after the
/// `)` of a parameter list, a catch clause, or a `for` head that declares it.
fn binding_end(text: &str, at: usize, name: &str) -> Option<usize> {
    let end = at + name.len();
    let before = text[..at].trim_end();
    let written_as_word =
        !text[..at].ends_with(is_word_char) && !text[end..].starts_with(is_word_char);
    let member = before.ends_with('.') && !before.ends_with("...");
    if !written_as_word || member {
        return None;
    }
    if text[end..].trim_start().starts_with("=>") {
        return Some(end);
    }
    let open = enclosing_open(text, at)?;
    let close = matching_close(text, open)?;
    declared_region(text, open, close)
        .filter(|region| declares(region, name))
        .map(|_| close + 1)
}

/// The index of the `}` that closes the body after a binding, when that body is a block
/// (`catch (e) { … }`, `(e) => { … }`, `for (const e of list) { … }`).
fn block_body_end(text: &str, after: usize) -> Option<usize> {
    let rest = text[after..].trim_start();
    let rest = rest.strip_prefix("=>").map_or(rest, str::trim_start);
    if !rest.starts_with('{') {
        return None;
    }
    brace_close(text, text.len() - rest.len())
}

/// The `}` that closes the `{` at `open`, past comments and quoted text.
/// shortcut: a brace inside a regular expression literal shifts the count; skip regex literals
/// when a project shows one.
fn brace_close(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let end = bytes.len();
    let mut depth = 0;
    let mut at = open;
    while at < end {
        if let Some(after) = comment_end(bytes, at, end) {
            at = after;
            continue;
        }
        match bytes[at] {
            b'\'' | b'"' | b'`' => at = quote_end(bytes, at, end)?,
            b'{' => depth += 1,
            b'}' if depth == 1 => return Some(at),
            b'}' => depth -= 1,
            _ => {}
        }
        at += 1;
    }
    None
}

pub(super) fn unmatched_brace_before(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut delimiters = Vec::new();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(after) = comment_end(bytes, at, bytes.len()) {
            at = after;
            continue;
        }
        match bytes[at] {
            b'\'' | b'"' | b'`' => at = quote_end(bytes, at, bytes.len())? + 1,
            b'/' => return None,
            b'(' | b'[' | b'{' => {
                delimiters.push((bytes[at], at));
                at += 1;
            }
            b')' | b']' | b'}' => {
                let (opening, _) = delimiters.pop()?;
                if !matches!(
                    (opening, bytes[at]),
                    (b'(', b')') | (b'[', b']') | (b'{', b'}')
                ) {
                    return None;
                }
                at += 1;
            }
            _ => at += 1,
        }
    }
    let (opening, position) = delimiters.pop()?;
    (opening == b'{').then_some(position)
}

/// The `(` of the innermost parentheses around `at`, within reach.
fn enclosing_open(text: &str, at: usize) -> Option<usize> {
    let start = at.saturating_sub(MAX_LIST);
    let mut depth = 0;
    for (offset, byte) in text.as_bytes()[start..at].iter().enumerate().rev() {
        match byte {
            b')' | b']' | b'}' => depth += 1,
            b'(' | b'[' | b'{' if depth > 0 => depth -= 1,
            b'(' => return Some(start + offset),
            _ => {}
        }
    }
    None
}

fn matching_close(text: &str, open: usize) -> Option<usize> {
    let mut depth = 0;
    for (offset, byte) in text.as_bytes()[open + 1..]
        .iter()
        .take(MAX_LIST)
        .enumerate()
    {
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth > 0 => depth -= 1,
            b')' => return Some(open + 1 + offset),
            b']' | b'}' => return None,
            _ => {}
        }
    }
    None
}

fn ends_with_word(text: &str, word: &str) -> bool {
    text.strip_suffix(word)
        .is_some_and(|rest| !rest.ends_with(is_word_char))
}

/// Whether `text` ends with the keyword `word`, not a method of that name (`p.catch(`).
fn ends_with_keyword(text: &str, word: &str) -> bool {
    ends_with_word(text, word) && !text[..text.len() - word.len()].trim_end().ends_with('.')
}

/// Whether `text`, which follows the `:` of a return type, reaches the `=>` of an arrow function
/// within `MAX_RETURN_TYPE` bytes. `<` and `>` nest like brackets, and comments and quoted text
/// are skipped. A `;`, `?`, or `,` outside brackets stops the look-ahead, and so do a closing
/// bracket with no opening one before it, a quote that does not close, and a line end that the
/// type does not go on past.
fn returns_to_arrow(text: &str) -> bool {
    let bytes = text.as_bytes();
    let end = bytes.len().min(MAX_RETURN_TYPE);
    let mut depth = 0;
    let mut last = None;
    let mut at = 0;
    while at < end {
        if let Some(after) = comment_end(bytes, at, end) {
            at = after;
            continue;
        }
        let byte = bytes[at];
        match byte {
            b'\'' | b'"' | b'`' => match quote_end(bytes, at, end) {
                Some(close) => at = close,
                None => return false,
            },
            b'=' if bytes.get(at + 1) == Some(&b'>') => {
                if depth == 0 {
                    return true;
                }
                at += 1;
            }
            b'(' | b'{' | b'[' | b'<' => depth += 1,
            b')' | b'}' | b']' | b'>' if depth == 0 => return false,
            b')' | b'}' | b']' | b'>' => depth -= 1,
            b';' | b'?' | b',' if depth == 0 => return false,
            b'\n' if depth == 0 && !type_continues(last, &text[at + 1..]) => return false,
            _ => {}
        }
        if !byte.is_ascii_whitespace() {
            last = Some(byte);
        }
        at += 1;
    }
    false
}

/// The index after the comment that starts at `at`. A line comment ends before its line end.
fn comment_end(bytes: &[u8], at: usize, end: usize) -> Option<usize> {
    let rest = &bytes[at..end];
    match rest {
        [b'/', b'/', ..] => Some(
            at + rest
                .iter()
                .position(|&byte| byte == b'\n')
                .unwrap_or(rest.len()),
        ),
        [b'/', b'*', ..] => Some(
            rest[2..]
                .windows(2)
                .position(|pair| pair == b"*/")
                .map_or(end, |close| at + close + 4),
        ),
        _ => None,
    }
}

/// The index of the quote that closes the one at `at`, past any escaped character.
fn quote_end(bytes: &[u8], at: usize, end: usize) -> Option<usize> {
    let mut index = at + 1;
    while index < end {
        match bytes[index] {
            b'\\' => index += 1,
            byte if byte == bytes[at] => return Some(index),
            _ => {}
        }
        index += 1;
    }
    None
}

/// Whether a return type goes on past a line end: no code of it comes before the line end, or a
/// `|` or `&` ends the line or starts the next one.
fn type_continues(last: Option<u8>, after: &str) -> bool {
    matches!(last, None | Some(b'|' | b'&')) || after.trim_start().starts_with(['|', '&'])
}

/// The code before a group that starts at the end of `text`. When the group starts its line, the
/// line comments on the lines above it are left out.
fn code_before(text: &str) -> &str {
    let mut before = text.trim_end();
    let line = text.rfind('\n').map_or(0, |at| at + 1);
    if !text[line..].trim().is_empty() {
        return before;
    }
    loop {
        let line = before.rfind('\n').map_or(0, |at| at + 1);
        match before[line..].find("//") {
            Some(at) => before = before[..line + at].trim_end(),
            None => return before,
        }
    }
}

/// The text of the parentheses `open..close` that declares names, when they are the parameters
/// of an unnamed function or an arrow function, a catch clause, or the declaration of a `for`
/// loop head. julie records the parameters of a named function itself.
fn declared_region(text: &str, open: usize, close: usize) -> Option<&str> {
    let before = code_before(&text[..open]);
    let inner = &text[open + 1..close];
    let after = text[close + 1..].trim_start();
    let function = ends_with_keyword(before, "function")
        || before
            .strip_suffix('*')
            .is_some_and(|generator| ends_with_keyword(generator.trim_end(), "function"));
    let call_like = before.ends_with([')', ']'])
        || (before.ends_with(is_word_char)
            && !ARROW_KEYWORDS
                .iter()
                .any(|word| ends_with_keyword(before, word)));
    let in_expression = ["&&", "||", "??"]
        .iter()
        .any(|operator| before.ends_with(operator));
    let typed_arrow = after
        .strip_prefix(':')
        .is_some_and(|returns| !in_expression && returns_to_arrow(returns))
        && (!call_like || ends_with_word(before, "async"));
    if ends_with_keyword(before, "catch") || function || after.starts_with("=>") || typed_arrow {
        return Some(inner);
    }
    let head = before.strip_suffix("await").map_or(before, str::trim_end);
    if !ends_with_keyword(head, "for") {
        return None;
    }
    let declaration = ["const", "let", "var"].iter().find_map(|keyword| {
        let rest = inner.trim_start().strip_prefix(keyword)?;
        rest.starts_with(char::is_whitespace).then_some(rest)
    })?;
    loop_pattern(declaration)
}

/// The part of `const <pattern> of <iterable>` before the `of` or `in`.
fn loop_pattern(declaration: &str) -> Option<&str> {
    let mut depth = 0;
    for (at, c) in declaration.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                let rest = declaration[at..].trim_start();
                let keyword = ["of", "in"].iter().find_map(|word| rest.strip_prefix(word));
                if keyword.is_some_and(|after| after.starts_with(char::is_whitespace)) {
                    return Some(&declaration[..at]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether the parameter list or pattern `list` declares `name` (`a, { b, c: [d] }, ...e`), but
/// not in a type annotation, a default value, or as an object key.
fn declares(list: &str, name: &str) -> bool {
    let bytes = list.as_bytes();
    let word =
        |byte: u8| byte >= 0x80 || byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$';
    let level = |open, want, skip, inert| Level {
        open,
        want,
        skip,
        inert,
        pending: None,
    };
    let mut levels = vec![level(b'(', true, false, false)];
    let mut at = 0;
    while at < bytes.len() {
        let byte = bytes[at];
        at += 1;
        let top = levels.last_mut().expect("the list level is never popped");
        match byte {
            b'\'' | b'"' | b'`' => {
                while at < bytes.len() && bytes[at] != byte {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                at += 1;
            }
            _ if word(byte) => {
                let start = at - 1;
                while at < bytes.len() && word(bytes[at]) {
                    at += 1;
                }
                if top.inert || top.skip || !top.want {
                    continue;
                }
                top.want = false;
                let ident = &list[start..at];
                if top.open == b'{' {
                    top.pending = Some(ident);
                } else if ident == name {
                    return true;
                }
            }
            b',' if !top.inert => {
                if settle(top, name) {
                    return true;
                }
                top.want = true;
                top.skip = false;
            }
            b':' if !top.inert && !top.skip => {
                if top.open == b'{' && top.pending.take().is_some() {
                    top.want = true;
                } else {
                    top.skip = true;
                }
            }
            b'=' if bytes.get(at) == Some(&b'>') => at += 1,
            b'=' if !top.inert && !top.skip => {
                if settle(top, name) {
                    return true;
                }
                top.skip = true;
            }
            b'{' | b'[' if !top.inert && !top.skip && top.want => {
                top.want = false;
                levels.push(level(byte, true, false, false));
            }
            b'{' | b'[' | b'(' => levels.push(level(byte, false, true, true)),
            b'<' if top.skip => levels.push(level(byte, false, true, true)),
            b'>' if top.open == b'<' => {
                levels.pop();
            }
            b')' | b']' | b'}' if levels.len() > 1 => {
                if let Some(mut closed) = levels.pop()
                    && settle(&mut closed, name)
                {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Whether the pending name of an object pattern is `name`, now that it is a binding.
fn settle(level: &mut Level, name: &str) -> bool {
    level.pending.take().is_some_and(|ident| ident == name)
}
