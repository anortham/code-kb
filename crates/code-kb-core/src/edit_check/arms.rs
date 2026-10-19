//! Whether a function needs a new arm when an enum gains a variant: it holds a `match`, `switch`,
//! or `when` in which every arm names a variant of the enum and no arm is a fallback. The
//! function's own lines are read, after comments and strings are blanked.

use crate::reference_report::variant_names::{code_only, is_identifier_char};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Site,
    Unresolved,
}

pub(super) struct EnumNames<'a> {
    pub enum_name: &'a str,
    pub variants: &'a [String],
    pub bare: bool,
}

/// What the function in `source` needs when the enum gains a variant. `Site`: it holds a
/// construct in which every arm names a variant of the enum and no arm is a fallback. Anything
/// else is `Unresolved`: a construct with an arm that names no variant or with a fallback arm, a
/// construct that could not be delimited, a function with no construct over the enum, or a
/// language with no reader.
pub(super) fn judge(language: &str, source: &[String], names: &EnumNames) -> Verdict {
    let code = code_only(&source.join("\n"), language);
    let exhaustive = constructs(language, &code).is_some_and(|found| {
        found.iter().any(|construct| {
            !construct.fallback
                && construct
                    .patterns
                    .iter()
                    .all(|pattern| names_variant(pattern, names))
        })
    });
    if exhaustive {
        Verdict::Site
    } else {
        Verdict::Unresolved
    }
}

struct Construct {
    patterns: Vec<String>,
    fallback: bool,
}

fn constructs(language: &str, code: &str) -> Option<Vec<Construct>> {
    match language {
        "rust" => Some(braced(code, "match", true)),
        "java" | "csharp" | "c" | "cpp" | "javascript" | "typescript" | "tsx" | "jsx" | "go"
        | "swift" | "dart" => Some(braced(code, "switch", false)),
        "kotlin" => Some(kotlin_whens(code)),
        "python" => Some(python_matches(code)),
        _ => None,
    }
}

/// Whether the variant is named by `pattern`: written after the enum's name (or `Self`) and `::`
/// or `.`, after a bare `.` (a Swift implicit member), or bare when the scope allows it. A C#
/// pattern that starts with `not` matches every other value, so it names no variant.
fn names_variant(pattern: &str, names: &EnumNames) -> bool {
    if starts_word(pattern.trim_start(), "not") {
        return false;
    }
    let mut at = 0;
    while at < pattern.len() {
        let Some(c) = pattern[at..].chars().next() else {
            break;
        };
        if !is_identifier_char(c) {
            at += c.len_utf8();
            continue;
        }
        let end = at
            + pattern[at..]
                .find(|c: char| !is_identifier_char(c))
                .unwrap_or(pattern.len() - at);
        let word = &pattern[at..end];
        if names.variants.iter().any(|variant| variant == word) {
            let before = pattern[..at].trim_end();
            let separator = if before.ends_with("::") {
                2
            } else {
                usize::from(before.ends_with('.'))
            };
            if separator == 0 {
                if names.bare {
                    return true;
                }
            } else {
                let owner = before[..before.len() - separator]
                    .trim_end()
                    .rsplit(|c: char| !is_identifier_char(c))
                    .next()
                    .unwrap_or("");
                if owner.is_empty() || owner == names.enum_name || owner == "Self" {
                    return true;
                }
            }
        }
        at = end;
    }
    false
}

fn word_positions(code: &str, word: &str) -> Vec<usize> {
    code.match_indices(word)
        .map(|(at, _)| at)
        .filter(|&at| {
            let before = code[..at].chars().next_back();
            let after = code[at + word.len()..].chars().next();
            !before.is_some_and(|c| is_identifier_char(c) || c == '.')
                && !after.is_some_and(is_identifier_char)
        })
        .collect()
}

/// The index of the first `{` after `from` that is not inside `(…)` or `[…]`; `None` when a `;`
/// or a `}` comes first.
fn open_brace(code: &str, from: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in code[from..].char_indices() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.checked_sub(1)?,
            '{' if depth == 0 => return Some(from + offset),
            ';' | '}' if depth == 0 => return None,
            _ => {}
        }
    }
    None
}

fn closing_brace(code: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in code[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// The body of every `keyword { … }` in `code`, read as labelled cases (`case X:`, `case X ->`)
/// or as `pattern => value` arms. `bindings` is set for Rust, which has only arms and lets a
/// lone lowercase name match everything.
fn braced(code: &str, keyword: &str, bindings: bool) -> Vec<Construct> {
    let mut found = Vec::new();
    for at in word_positions(code, keyword) {
        let body = open_brace(code, at + keyword.len())
            .and_then(|open| closing_brace(code, open).map(|close| &code[open + 1..close]));
        let Some(body) = body else {
            continue;
        };
        let construct = if bindings {
            fat_arrow_arms(body, true)
        } else {
            labelled(body).or_else(|| fat_arrow_arms(body, false))
        };
        found.extend(construct);
    }
    found
}

/// The labels of a `switch` body at its own level, or `None` when the body has none.
fn labelled(body: &str) -> Option<Construct> {
    let mut patterns = Vec::new();
    let mut fallback = false;
    let mut seen = false;
    let mut depth = 0usize;
    let mut at = 0;
    while at < body.len() {
        let Some(c) = body[at..].chars().next() else {
            break;
        };
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0 && is_identifier_char(c) => {
                let end = at
                    + body[at..]
                        .find(|c: char| !is_identifier_char(c))
                        .unwrap_or(body.len() - at);
                let word = &body[at..end];
                let standalone = !body[..at].ends_with(|c: char| c == '.' || is_identifier_char(c));
                if standalone && word == "case" {
                    let stop = label_end(body, end);
                    let label = &body[end..stop];
                    fallback |= label
                        .split(|c: char| !is_identifier_char(c))
                        .any(|word| word == "default");
                    patterns.push(label.to_string());
                    seen = true;
                    at = stop;
                    continue;
                }
                if standalone && word == "default" {
                    let after = body[end..].trim_start();
                    if after.starts_with(':') || after.starts_with("->") {
                        fallback = true;
                        seen = true;
                    }
                }
                at = end;
                continue;
            }
            _ => {}
        }
        at += c.len_utf8();
    }
    seen.then_some(Construct { patterns, fallback })
}

/// The index of the `:` (not part of `::`), `->`, or `=>` that ends the label that starts at `from`.
fn label_end(body: &str, from: usize) -> usize {
    let bytes = body.as_bytes();
    let mut depth = 0usize;
    for (offset, &byte) in bytes[from..].iter().enumerate() {
        let at = from + offset;
        match byte {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b':' if depth == 0
                && bytes.get(at + 1) != Some(&b':')
                && !(at > 0 && bytes[at - 1] == b':') =>
            {
                return at;
            }
            b'-' | b'=' if depth == 0 && bytes.get(at + 1) == Some(&b'>') => return at,
            _ => {}
        }
    }
    body.len()
}

fn fat_arrow_arms(body: &str, bindings: bool) -> Option<Construct> {
    let mut patterns = Vec::new();
    let mut fallback = false;
    let mut at = 0;
    loop {
        at += body[at..]
            .find(|c: char| !(c.is_whitespace() || c == ','))
            .unwrap_or(body.len() - at);
        if at >= body.len() {
            break;
        }
        let arrow = find_at_depth_zero(body, at, "=>")?;
        let head = body[at..arrow].trim();
        let (pattern, guarded) = split_guard(head);
        fallback |= !guarded && is_irrefutable(pattern, bindings);
        patterns.push(pattern.to_string());
        at = arm_end(body, arrow + 2);
    }
    (!patterns.is_empty()).then_some(Construct { patterns, fallback })
}

fn find_at_depth_zero(text: &str, from: usize, needle: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, c) in text[from..].char_indices() {
        let at = from + offset;
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.checked_sub(1)?,
            _ if depth == 0 && text[at..].starts_with(needle) => return Some(at),
            _ => {}
        }
    }
    None
}

fn starts_word(text: &str, word: &str) -> bool {
    text.strip_prefix(word)
        .is_some_and(|rest| !rest.starts_with(is_identifier_char))
}

/// The index after the arm whose value starts at `from`: after the `,` that ends it, or, for a
/// value that is a block or begins with a block keyword, after its closing `}` (a comma may
/// follow) unless `else`, `.`, or `?` continues it.
fn arm_end(body: &str, from: usize) -> usize {
    let rest = &body[from..];
    let first = rest.trim_start();
    let block_like = first.starts_with('{')
        || ["if", "match", "loop", "while", "for", "unsafe"]
            .iter()
            .any(|keyword| starts_word(first, keyword));
    let mut depth = 0usize;
    for (offset, c) in rest.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 && block_like {
                    let tail = rest[offset + 1..].trim_start();
                    if !(starts_word(tail, "else") || tail.starts_with(['.', '?'])) {
                        return from + offset + 1;
                    }
                }
            }
            ',' if depth == 0 => return from + offset + 1,
            _ => {}
        }
    }
    body.len()
}

/// The pattern of an arm head and whether a guard (`if`, `when`) follows it.
fn split_guard(head: &str) -> (&str, bool) {
    let mut depth = 0usize;
    for (at, c) in head.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            _ if depth == 0
                && c.is_whitespace()
                && (starts_word(head[at..].trim_start(), "if")
                    || starts_word(head[at..].trim_start(), "when")) =>
            {
                return (head[..at].trim_end(), true);
            }
            _ => {}
        }
    }
    (head, false)
}

/// Whether some alternative of `pattern` matches every value: `_`, `name @ _`, and, when
/// `bindings` is set, a lone lowercase name (`other`, `ref other`).
fn is_irrefutable(pattern: &str, bindings: bool) -> bool {
    let mut alternatives = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, c) in pattern.char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            '|' if depth == 0 => {
                alternatives.push(&pattern[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    alternatives.push(&pattern[start..]);
    alternatives.iter().any(|alternative| {
        let alternative = alternative.trim();
        let tested = alternative
            .rsplit_once('@')
            .map_or(alternative, |(_, rest)| rest.trim());
        tested == "_" || (bindings && is_lowercase_binding(tested))
    })
}

fn is_lowercase_binding(text: &str) -> bool {
    let mut words = text.split_whitespace().collect::<Vec<_>>();
    while words.len() > 1 && matches!(words[0], "ref" | "mut") {
        words.remove(0);
    }
    let [word] = words.as_slice() else {
        return false;
    };
    word.chars().all(is_identifier_char)
        && word.starts_with(|c: char| c.is_lowercase() || c == '_')
        && !matches!(*word, "true" | "false" | "_")
}

/// Every `when { … }` of Kotlin code: each arm is `conditions -> value`, one per line or after a `;`.
fn kotlin_whens(code: &str) -> Vec<Construct> {
    let mut found = Vec::new();
    for at in word_positions(code, "when") {
        let body = open_brace(code, at + 4)
            .and_then(|open| closing_brace(code, open).map(|close| &code[open + 1..close]));
        let Some(body) = body else {
            continue;
        };
        let mut patterns = Vec::new();
        let mut fallback = false;
        let mut depth = 0usize;
        let mut head_start = 0;
        let bytes = body.as_bytes();
        for (at, &byte) in bytes.iter().enumerate() {
            match byte {
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth = depth.saturating_sub(1),
                b'\n' | b';' if depth == 0 => head_start = at + 1,
                b'-' if depth == 0 && bytes.get(at + 1) == Some(&b'>') => {
                    let head = body[head_start..at].trim();
                    fallback |= head == "else";
                    patterns.push(head.to_string());
                    head_start = at + 2;
                }
                _ => {}
            }
        }
        if !patterns.is_empty() {
            found.push(Construct { patterns, fallback });
        }
    }
    found
}

/// Every `match subject:` statement of Python code, read through the `case` lines one level
/// inside it.
fn python_matches(code: &str) -> Vec<Construct> {
    let mut found = Vec::new();
    let lines: Vec<&str> = code.lines().collect();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        let Some(subject) = trimmed.strip_prefix("match ") else {
            continue;
        };
        if !trimmed.ends_with(':') || subject.trim_start().starts_with('=') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        let mut case_indent = None;
        let mut patterns = Vec::new();
        let mut fallback = false;
        for next in &lines[index + 1..] {
            let text = next.trim();
            if text.is_empty() {
                continue;
            }
            let next_indent = next.len() - next.trim_start().len();
            if next_indent <= indent {
                break;
            }
            let Some(case) = text.strip_prefix("case ") else {
                continue;
            };
            if *case_indent.get_or_insert(next_indent) != next_indent {
                continue;
            }
            let colon = find_at_depth_zero(case, 0, ":").unwrap_or(case.len());
            let (pattern, guarded) = split_guard(case[..colon].trim());
            fallback |= !guarded && is_irrefutable(pattern, true);
            patterns.push(pattern.to_string());
        }
        if !patterns.is_empty() {
            found.push(Construct { patterns, fallback });
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verdict_with(language: &str, source: &str, bare: bool) -> Verdict {
        let lines: Vec<String> = source.lines().map(str::to_string).collect();
        let variants = ["Red", "Green"].map(str::to_string);
        judge(
            language,
            &lines,
            &EnumNames {
                enum_name: "Color",
                variants: &variants,
                bare,
            },
        )
    }

    fn verdict(language: &str, source: &str) -> Verdict {
        verdict_with(language, source, false)
    }

    #[test]
    fn a_rust_match_with_no_fallback_arm_is_a_site() {
        let source = "fn name(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n        Color::Green => 2,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Site);
    }

    #[test]
    fn a_rust_match_with_a_wildcard_or_a_bound_name_is_unresolved() {
        let wildcard = "fn f(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n        _ => 0,\n    }\n}";
        let bound = "fn f(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n        other => other as i32,\n    }\n}";
        let either = "fn f(c: Color) -> i32 {\n    match c {\n        Color::Red | Color::Green => 1,\n        ref other | mut other => 2,\n    }\n}";
        assert_eq!(verdict("rust", wildcard), Verdict::Unresolved);
        assert_eq!(verdict("rust", bound), Verdict::Unresolved);
        assert_eq!(verdict("rust", either), Verdict::Unresolved);
    }

    #[test]
    fn a_guarded_wildcard_or_a_wildcard_with_a_literal_names_no_variant_and_is_unresolved() {
        let guarded = "fn f(c: Color, ok: bool) -> i32 {\n    match c {\n        Color::Red => 1,\n        _ if ok => 2,\n        Color::Green => 3,\n    }\n}";
        let literal = "fn f(c: Color, b: bool) -> i32 {\n    match (c, b) {\n        (Color::Red, true) => 1,\n        (_, false) => 2,\n    }\n}";
        assert_eq!(verdict("rust", guarded), Verdict::Unresolved);
        assert_eq!(verdict("rust", literal), Verdict::Unresolved);
    }

    #[test]
    fn a_rust_function_that_only_builds_or_tests_a_variant_is_unresolved() {
        let source = "fn f(c: Color) -> bool {\n    let made = Color::Red;\n    if let Color::Green = c { return true; }\n    matches!(c, Color::Red) || c == made\n}";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_match_on_another_enum_that_shares_a_variant_name_is_unresolved() {
        let source = "fn f(s: Shape) -> i32 {\n    match s {\n        Shape::Red => 1,\n        Shape::Blue => 2,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_nested_match_is_judged_on_its_own() {
        let source = "fn f(c: Color, n: i32) -> i32 {\n    match n {\n        0 => match c {\n            Color::Red => 1,\n            Color::Green => 2,\n        },\n        _ => 3,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Site);
    }

    #[test]
    fn rust_arms_without_commas_after_blocks_are_all_read() {
        let source = "fn f(c: Color) {\n    match c {\n        Color::Red => {\n            a();\n        }\n        Color::Green => if x { b() } else { c() }\n        other => d(other),\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_lifetime_on_the_line_of_a_match_does_not_hide_it() {
        let source = "fn f(c: Color) -> &'static str {\n    let s: &'static str = match c {\n        Color::Red => \"r\",\n        Color::Green => \"g\",\n    };\n    s\n}";
        assert_eq!(verdict("rust", source), Verdict::Site);
    }

    #[test]
    fn variants_in_comments_and_strings_do_not_count() {
        let source = "fn f(c: Color) -> i32 {\n    match c {\n        // Color::Red => 1,\n        _ => \"Color::Green\".len() as i32,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_bare_name_counts_only_when_the_scope_imports_the_variants() {
        let source = "fn f(c: Color) -> i32 {\n    match c {\n        Red => 1,\n        Green => 2,\n    }\n}";
        assert_eq!(verdict_with("rust", source, false), Verdict::Unresolved);
        assert_eq!(verdict_with("rust", source, true), Verdict::Site);
    }

    #[test]
    fn self_names_a_variant() {
        let source = "fn f(&self) -> i32 {\n    match self {\n        Self::Red => 1,\n        Self::Green => 2,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Site);
    }

    #[test]
    fn a_rust_name_spelled_case_is_a_binding_and_not_a_label() {
        let source = "fn f(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n        case => 0,\n    }\n}";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_match_that_never_closes_is_unresolved() {
        let source = "fn f(c: Color) -> i32 {\n    match c {\n        Color::Red => 1,\n";
        assert_eq!(verdict("rust", source), Verdict::Unresolved);
    }

    #[test]
    fn a_language_with_no_reader_is_unresolved() {
        let source = "def f(c)\n  case c\n  when Color::Red then 1\n  end\nend";
        assert_eq!(verdict("ruby", source), Verdict::Unresolved);
    }

    #[test]
    fn a_java_switch_is_judged_by_its_default_label() {
        let open = "int f(Color c) {\n    switch (c) {\n        case Red:\n            return 1;\n        case Green:\n            return 2;\n    }\n    return 0;\n}";
        let closed = "int f(Color c) {\n    switch (c) {\n        case Red:\n            return 1;\n        default:\n            return 0;\n    }\n}";
        let arrow = "int f(Color c) {\n    return switch (c) {\n        case Red -> 1;\n        default -> 0;\n    };\n}";
        let null = "int f(Color c) {\n    return switch (c) {\n        case Red -> 1;\n        case null, default -> 0;\n    };\n}";
        assert_eq!(verdict_with("java", open, true), Verdict::Site);
        assert_eq!(verdict_with("java", closed, true), Verdict::Unresolved);
        assert_eq!(verdict_with("java", arrow, true), Verdict::Unresolved);
        assert_eq!(verdict_with("java", null, true), Verdict::Unresolved);
    }

    #[test]
    fn a_switch_inside_a_case_body_is_judged_apart_from_the_outer_one() {
        let source = "int f(int n, Color c) {\n    switch (n) {\n        case 1:\n            switch (c) {\n                case Red:\n                    return 1;\n            }\n            return 2;\n        default:\n            return 0;\n    }\n}";
        assert_eq!(verdict_with("java", source, true), Verdict::Site);
    }

    #[test]
    fn a_java_equality_test_on_a_variant_is_unresolved() {
        let source = "int f(Color c) {\n    return c == Color.Red ? 1 : 0;\n}";
        assert_eq!(verdict_with("java", source, true), Verdict::Unresolved);
    }

    #[test]
    fn a_typescript_switch_is_judged_by_its_default_clause() {
        let open = "function f(c: Color): string {\n  switch (c) {\n    case Color.Red:\n      return \"r\";\n    case Color.Green:\n      return \"g\";\n  }\n  return \"\";\n}";
        let closed = "function f(c: Color): string {\n  switch (c) {\n    case Color.Red:\n      return \"r\";\n    default:\n      return \"\";\n  }\n}";
        assert_eq!(verdict("typescript", open), Verdict::Site);
        assert_eq!(verdict("typescript", closed), Verdict::Unresolved);
    }

    #[test]
    fn a_cpp_case_with_a_scope_operator_is_read_to_its_colon() {
        let source = "int f(Color c) {\n    switch (c) {\n        case Color::Red:\n            return 1;\n        case Color::Green:\n            return 2;\n    }\n    return 0;\n}";
        assert_eq!(verdict("cpp", source), Verdict::Site);
    }

    #[test]
    fn a_csharp_switch_expression_is_judged_by_its_wildcard_arm() {
        let open = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        Color.Green => 2,\n    };\n}";
        let closed = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        _ => 0,\n    };\n}";
        let guarded = "int F(Color c, bool ok) {\n    return c switch {\n        Color.Red when ok => 1,\n        Color.Red => 0,\n        Color.Green => 2,\n    };\n}";
        let guarded_wildcard = "int F(Color c, bool ok) {\n    return c switch {\n        Color.Red => 1,\n        _ when ok => 0,\n        Color.Green => 2,\n    };\n}";
        assert_eq!(verdict("csharp", open), Verdict::Site);
        assert_eq!(verdict("csharp", closed), Verdict::Unresolved);
        assert_eq!(verdict("csharp", guarded), Verdict::Site);
        assert_eq!(verdict("csharp", guarded_wildcard), Verdict::Unresolved);
    }

    #[test]
    fn a_swift_switch_names_a_variant_with_a_leading_dot() {
        let open = "func f(c: Color) -> Int {\n    switch c {\n    case .Red:\n        return 1\n    case .Green:\n        return 2\n    }\n}";
        let closed = "func f(c: Color) -> Int {\n    switch c {\n    case .Red:\n        return 1\n    default:\n        return 0\n    }\n}";
        assert_eq!(verdict("swift", open), Verdict::Site);
        assert_eq!(verdict("swift", closed), Verdict::Unresolved);
    }

    #[test]
    fn a_kotlin_when_is_judged_by_its_else_arm() {
        let open = "fun f(c: Color): Int {\n    return when (c) {\n        Color.Red -> 1\n        Color.Green -> 2\n    }\n}";
        let closed = "fun f(c: Color): Int {\n    return when (c) {\n        Color.Red -> 1\n        else -> 0\n    }\n}";
        let listed = "fun f(c: Color): Int {\n    return when (c) {\n        Color.Red, Color.Green -> { 1 }\n    }\n}";
        let other = "fun f(c: Color): Int {\n    return when (c) {\n        Color.Red, Color.Green -> { 1 }\n        is Other -> 2\n    }\n}";
        assert_eq!(verdict("kotlin", open), Verdict::Site);
        assert_eq!(verdict("kotlin", closed), Verdict::Unresolved);
        assert_eq!(verdict("kotlin", listed), Verdict::Site);
        assert_eq!(verdict("kotlin", other), Verdict::Unresolved);
    }

    #[test]
    fn a_python_match_is_judged_by_its_wildcard_case() {
        let open = "def f(c):\n    match c:\n        case Color.Red:\n            return 1\n        case Color.Green | Color.Blue:\n            return 2\n";
        let closed = "def f(c):\n    match c:\n        case Color.Red:\n            return 1\n        case _:\n            return 0\n";
        let guarded = "def f(c, ok):\n    match c:\n        case Color.Red if ok:\n            return 1\n        case Color.Red | Color.Green:\n            return 0\n";
        let guarded_wildcard = "def f(c, ok):\n    match c:\n        case Color.Red:\n            return 1\n        case _ if ok:\n            return 0\n";
        assert_eq!(verdict("python", open), Verdict::Site);
        assert_eq!(verdict("python", closed), Verdict::Unresolved);
        assert_eq!(verdict("python", guarded), Verdict::Site);
        assert_eq!(verdict("python", guarded_wildcard), Verdict::Unresolved);
    }

    #[test]
    fn a_rust_arm_that_names_no_variant_makes_the_match_unresolved() {
        let some = "fn f(o: Option<Color>) -> i32 {\n    match o {\n        Some(Color::Red) => 1,\n        Some(_) => 2,\n        None => 0,\n    }\n}";
        let wildcards = "fn f(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, Color::Red) => 1,\n        (_, _) => 2,\n    }\n}";
        let name_last = "fn f(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, Color::Red) => 1,\n        (_, Color::Green) => 2,\n        (_, _) => 3,\n    }\n}";
        let none = "fn f(o: Option<Color>) -> i32 {\n    match o {\n        Some(Color::Red) => 1,\n        Some(Color::Green) => 2,\n        None => 0,\n    }\n}";
        assert_eq!(verdict("rust", some), Verdict::Unresolved);
        assert_eq!(verdict("rust", wildcards), Verdict::Unresolved);
        assert_eq!(verdict("rust", name_last), Verdict::Unresolved);
        assert_eq!(verdict("rust", none), Verdict::Unresolved);
    }

    #[test]
    fn a_rust_match_is_a_site_when_every_arm_names_a_variant_with_or_without_a_guard() {
        let guarded = "fn f(c: Color, ok: bool) -> i32 {\n    match c {\n        Color::Red if ok => 1,\n        Color::Red => 2,\n        Color::Green => 3,\n    }\n}";
        let partial = "fn f(a: Color, b: i32) -> i32 {\n    match (a, b) {\n        (Color::Red, _) => 1,\n        (Color::Green, _) => 2,\n    }\n}";
        let listed = "fn f(a: Color, b: Color) -> i32 {\n    match (a, b) {\n        (Color::Red, Color::Red) => 1,\n        (Color::Red, Color::Green) => 2,\n        (Color::Green, _) => 3,\n    }\n}";
        assert_eq!(verdict("rust", guarded), Verdict::Site);
        assert_eq!(verdict("rust", partial), Verdict::Site);
        assert_eq!(verdict("rust", listed), Verdict::Site);
    }

    #[test]
    fn a_csharp_arm_that_binds_a_name_or_names_a_type_makes_the_switch_unresolved() {
        let var = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        var other => 2,\n    };\n}";
        let typed = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        Color other => 2,\n    };\n}";
        let tuple = "int F(Color a, Color b) {\n    return (a, b) switch {\n        (Color.Red, Color.Red) => 1,\n        (_, _) => 2,\n    };\n}";
        let alternatives = "int F(Color c) {\n    return c switch {\n        Color.Red or Color.Green => 1,\n    };\n}";
        assert_eq!(verdict("csharp", var), Verdict::Unresolved);
        assert_eq!(verdict("csharp", typed), Verdict::Unresolved);
        assert_eq!(verdict("csharp", tuple), Verdict::Unresolved);
        assert_eq!(verdict("csharp", alternatives), Verdict::Site);
    }

    #[test]
    fn a_csharp_pattern_that_starts_with_not_names_no_variant() {
        let expression = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        not Color.Red => 2,\n    };\n}";
        let group = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        not (Color.Red or Color.Green) => 2,\n    };\n}";
        let statement = "int F(Color c) {\n    switch (c) {\n        case Color.Red:\n            return 1;\n        case not Color.Red:\n            return 2;\n    }\n    return 0;\n}";
        let plain = "int F(Color c) {\n    return c switch {\n        Color.Red => 1,\n        Color.Green => 2,\n    };\n}";
        assert_eq!(verdict("csharp", expression), Verdict::Unresolved);
        assert_eq!(verdict("csharp", group), Verdict::Unresolved);
        assert_eq!(verdict("csharp", statement), Verdict::Unresolved);
        assert_eq!(verdict("csharp", plain), Verdict::Site);
    }

    #[test]
    fn a_python_case_that_names_no_variant_makes_the_match_unresolved() {
        let tuple = "def f(a, b):\n    match (a, b):\n        case (Color.Red, Color.Red):\n            return 1\n        case (_, _):\n            return 2\n";
        let sequence = "def f(c):\n    match c:\n        case Color.Red:\n            return 1\n        case [x, y]:\n            return 2\n";
        let partial = "def f(a, b):\n    match (a, b):\n        case (Color.Red, _):\n            return 1\n        case (Color.Green, _):\n            return 2\n";
        assert_eq!(verdict("python", tuple), Verdict::Unresolved);
        assert_eq!(verdict("python", sequence), Verdict::Unresolved);
        assert_eq!(verdict("python", partial), Verdict::Site);
    }

    #[test]
    fn a_switch_label_that_names_no_variant_makes_it_unresolved() {
        let source = "function f(c, n) {\n  switch (c) {\n    case Color.Red:\n      return 1;\n    case 7:\n      return 2;\n  }\n}";
        assert_eq!(verdict("typescript", source), Verdict::Unresolved);
    }

    #[test]
    fn a_function_with_no_match_over_the_enum_is_unresolved() {
        let record = "export const names: Record<Color, string> = {\n  [Color.Red]: \"r\",\n  [Color.Green]: \"g\",\n};";
        let chain = "function f(c: Color): number {\n  if (c === Color.Red) return 1;\n  if (c === Color.Green) return 2;\n  return 0;\n}";
        let dict = "def f(k):\n    return {Color.Red: 'r', Color.Green: 'g'}[k]\n";
        assert_eq!(verdict("typescript", record), Verdict::Unresolved);
        assert_eq!(verdict("typescript", chain), Verdict::Unresolved);
        assert_eq!(verdict("python", dict), Verdict::Unresolved);
    }

    #[test]
    fn a_python_name_called_match_is_not_a_match_statement() {
        let source =
            "def f(c):\n    match = re.match(\"x\", c)\n    return Color.Red if match else None\n";
        assert_eq!(verdict("python", source), Verdict::Unresolved);
    }
}
