//! Finds where a JavaScript or TypeScript file binds a name as a callback parameter, a loop
//! variable, or a catch parameter. julie records no symbol for these, so only the source text
//! shows that they hide an import.

use std::rc::Rc;

use super::js_modules::BindingSites;
use super::{Accounting, NameRow, QueryError, is_word_char, language_family};

/// The longest parameter list or loop head read, in bytes.
const MAX_LIST: usize = 400;

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
        let bindings = self.js_binding_sites(&row.path, head);
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
            .find(|&&binding| (caller_start, 0) <= binding && binding < site)
            .map(|&(line, _)| line))
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

fn binding_sites(text: &str, name: &str) -> Vec<(usize, usize)> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    text.match_indices(name)
        .filter(|&(at, _)| binds(text, at, name))
        .map(|(at, _)| {
            let line = line_starts.partition_point(|&start| start <= at);
            (line, at - line_starts[line - 1])
        })
        .collect()
}

/// Whether the word `name` at `at` is a binding: `name =>`, or a name of a parameter list, a
/// catch clause, or a `for` head that encloses it.
fn binds(text: &str, at: usize, name: &str) -> bool {
    let end = at + name.len();
    let before = text[..at].trim_end();
    let written_as_word =
        !text[..at].ends_with(is_word_char) && !text[end..].starts_with(is_word_char);
    let member = before.ends_with('.') && !before.ends_with("...");
    if !written_as_word || member {
        return false;
    }
    text[end..].trim_start().starts_with("=>")
        || enclosing_open(text, at)
            .and_then(|open| Some((open, matching_close(text, open)?)))
            .and_then(|(open, close)| declared_region(text, open, close))
            .is_some_and(|region| declares(region, name))
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

/// The text of the parentheses `open..close` that declares names, when they are the parameters
/// of an unnamed function or an arrow function, a catch clause, or the declaration of a `for`
/// loop head. julie records the parameters of a named function itself.
fn declared_region(text: &str, open: usize, close: usize) -> Option<&str> {
    let before = text[..open].trim_end();
    let inner = &text[open + 1..close];
    let after = text[close + 1..].trim_start();
    let function = ends_with_word(before, "function")
        || before
            .strip_suffix('*')
            .is_some_and(|generator| ends_with_word(generator.trim_end(), "function"));
    let call_like = before.ends_with([')', ']']) || before.ends_with(is_word_char);
    let typed_arrow = after.starts_with(':')
        && (!call_like || ends_with_word(before, "async"))
        && after
            .split_once("=>")
            .is_some_and(|(returns, _)| !returns.contains([';', '{', '}']));
    if ends_with_word(before, "catch") || function || after.starts_with("=>") || typed_arrow {
        return Some(inner);
    }
    let head = before.strip_suffix("await").map_or(before, str::trim_end);
    if !ends_with_word(head, "for") {
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
