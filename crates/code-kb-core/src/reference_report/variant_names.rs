use std::collections::{BTreeMap, BTreeSet};

pub(crate) use super::sections::is_identifier_char;
use super::sections::{block_comment_end, char_literal_end, quoted_end, raw_string_end};

const DECLARATION_WORDS: [&str; 8] = [
    "struct", "enum", "fn", "type", "mod", "trait", "union", "as",
];

/// `text` with every comment, string literal, and character literal replaced by spaces, newlines
/// kept, so offsets and line numbers stay. Python and Ruby comments start with `#`; every other
/// language here uses `//` and `/* */` (nested in Rust, Kotlin, and Swift). A quote that does not
/// close ends at its line, except a triple quote, and a backtick string in JavaScript, TypeScript,
/// and Go, which run on. Rust tells a lifetime from a character literal and reads raw strings.
pub(crate) fn code_only(text: &str, language: &str) -> String {
    let python = language == "python";
    let hash = python || language == "ruby";
    let rust = language == "rust";
    let nested = matches!(language, "rust" | "kotlin" | "swift");
    let backticks = matches!(language, "javascript" | "typescript" | "tsx" | "jsx" | "go");
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let rest = &bytes[at..];
        let end = if (hash && rest[0] == b'#') || (!hash && rest.starts_with(b"//")) {
            at + rest.iter().position(|b| *b == b'\n').unwrap_or(rest.len())
        } else if !hash && rest.starts_with(b"/*") {
            block_comment_end(bytes, at, nested)
        } else if rest.starts_with(b"\"\"\"") || (python && rest.starts_with(b"'''")) {
            let fence = &rest[..3];
            at + 3
                + rest[3..]
                    .windows(3)
                    .position(|window| window == fence)
                    .map_or(rest.len() - 3, |close| close + 3)
        } else if rust
            && matches!(rest[0], b'r' | b'b' | b'c')
            && !out
                .last()
                .is_some_and(|b: &u8| is_identifier_char(*b as char))
        {
            match raw_string_end(&text[at..], 0) {
                Some(end) => at + end,
                None => {
                    out.push(bytes[at]);
                    at += 1;
                    continue;
                }
            }
        } else if rest[0] == b'\'' && rust {
            match char_literal_end(&text[at..], 0) {
                Some(end) => at + end,
                None => {
                    out.push(bytes[at]);
                    at += 1;
                    continue;
                }
            }
        } else if rest[0] == b'"' || rest[0] == b'\'' {
            quoted_end(bytes, at)
        } else if backticks && rest[0] == b'`' {
            at + 1
                + rest[1..]
                    .iter()
                    .position(|b| *b == b'`')
                    .map_or(rest.len() - 1, |close| close + 1)
        } else {
            out.push(bytes[at]);
            at += 1;
            continue;
        };
        out.extend(
            bytes[at..end]
                .iter()
                .map(|byte| if *byte == b'\n' { b'\n' } else { b' ' }),
        );
        at = end;
    }
    String::from_utf8(out).unwrap_or_default()
}

/// The variants of the enum `enum_name` that a Rust `use` item brings in by name: the listed
/// ones (`use a::Kind::{Circle, Square};`) or all of `variants` for a glob (`use a::Kind::*;`).
/// An item renamed with `as` binds another name and brings none.
pub(crate) fn imported_variants(
    signature: &str,
    enum_name: &str,
    variants: &[String],
) -> Vec<String> {
    let Some(at) = signature.find("use ") else {
        return Vec::new();
    };
    let tree = signature[at + 4..].trim().trim_end_matches(';');
    let mut leaves = Vec::new();
    use_leaves(tree, &mut Vec::new(), &mut leaves);
    let mut names = Vec::new();
    for (path, leaf) in leaves {
        if path.last().map(String::as_str) != Some(enum_name) {
            continue;
        }
        if leaf == "*" {
            names.extend(variants.iter().cloned());
        } else if variants.contains(&leaf) {
            names.push(leaf);
        }
    }
    names
}

/// The `(path, name)` of each leaf of a `use` tree, where `path` is the segments above it.
fn use_leaves(tree: &str, prefix: &mut Vec<String>, leaves: &mut Vec<(Vec<String>, String)>) {
    for item in split_top_level_commas(tree) {
        let item = item.trim();
        if let Some(open) = item.find('{') {
            let head: Vec<String> = item[..open]
                .split("::")
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(str::to_string)
                .collect();
            let inner = &item[open + 1..];
            let inner = inner.trim_end().strip_suffix('}').unwrap_or(inner);
            let depth = prefix.len();
            prefix.extend(head);
            use_leaves(inner, prefix, leaves);
            prefix.truncate(depth);
        } else if !item.is_empty() && !item.contains(" as ") {
            let mut segments: Vec<&str> = item
                .split("::")
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .collect();
            if let Some(leaf) = segments.pop() {
                let mut path = prefix.clone();
                path.extend(segments.into_iter().map(str::to_string));
                leaves.push((path, leaf.to_string()));
            }
        }
    }
}

fn split_top_level_commas(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (at, c) in text.char_indices() {
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..at]);
                start = at + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

/// Whether the text before a name on its line leaves the name in a `case` label: `case` comes
/// right before it, or before the comma-separated names that come before it (`case A, B`).
pub(crate) fn follows_case(before: &str) -> bool {
    let mut text = before.trim_end();
    while let Some(rest) = text.strip_suffix(',') {
        text = rest
            .trim_end()
            .trim_end_matches(is_identifier_char)
            .trim_end();
    }
    text.strip_suffix("case")
        .is_some_and(|rest| !rest.ends_with(is_identifier_char))
}

/// The first line (0-based) of each of `names` that stands bare in `code`: not after `::` or `.`,
/// not before `::` or `!`, not the name of an item being declared, and not inside a `use` item.
pub(crate) fn bare_names(code: &str, names: &BTreeSet<&str>) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    let mut line = 0;
    let mut previous = "";
    let mut at = 0;
    while at < code.len() {
        let Some(c) = code[at..].chars().next() else {
            break;
        };
        if c == '\n' {
            line += 1;
            at += 1;
            continue;
        }
        if !is_identifier_char(c) {
            if !c.is_whitespace() {
                previous = "";
            }
            at += c.len_utf8();
            continue;
        }
        let end = at
            + code[at..]
                .find(|c: char| !is_identifier_char(c))
                .unwrap_or(code.len() - at);
        let word = &code[at..end];
        if word == "use" && starts_a_statement(&code[..at]) {
            let stop = code[end..]
                .find(';')
                .map_or(code.len(), |close| end + close + 1);
            line += code[end..stop].matches('\n').count();
            at = stop;
            previous = "";
            continue;
        }
        if names.contains(word)
            && stands_bare(code, at, end)
            && !DECLARATION_WORDS.contains(&previous)
        {
            found.entry(word.to_string()).or_insert(line);
        }
        previous = word;
        at = end;
    }
    found
}

fn starts_a_statement(before: &str) -> bool {
    before
        .trim_end()
        .chars()
        .next_back()
        .is_none_or(|c| matches!(c, ';' | '{' | '}'))
}

fn stands_bare(code: &str, start: usize, end: usize) -> bool {
    let before = code[..start].trim_end();
    let after = code[end..].trim_start();
    !(before.ends_with("::")
        || before.ends_with('.')
        || after.starts_with("::")
        || after.starts_with('!'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants() -> Vec<String> {
        ["Circle", "Square", "Triangle"]
            .map(str::to_string)
            .to_vec()
    }

    #[test]
    fn comments_strings_and_characters_become_spaces_and_lines_stay() {
        let code = code_only("a // b\n\"c\" 'd' /* e\nf */ g", "java");
        assert_eq!(
            code,
            format!("a{}\n{}\n{}g", " ".repeat(5), " ".repeat(12), " ".repeat(5))
        );
    }

    #[test]
    fn a_rust_lifetime_is_code_and_a_character_literal_is_not() {
        let code = code_only(
            "let s: &'static str = match c { 'x' => 1, _ => 2 };",
            "rust",
        );
        assert!(code.contains("'static str = match c {"), "{code}");
        assert!(!code.contains("'x'"), "{code}");
    }

    #[test]
    fn python_comments_start_with_a_hash_and_slashes_are_code() {
        let code = code_only("a = b // c  # d\n", "python");
        assert_eq!(code, "a = b // c     \n");
    }

    #[test]
    fn ruby_comments_start_with_a_hash_outside_a_string() {
        let code = code_only("p = Point.new(\"#{x}\") # Point.new(2)\n", "ruby");
        assert_eq!(
            code,
            format!("p = Point.new({}) {}\n", " ".repeat(6), " ".repeat(14))
        );
    }

    #[test]
    fn a_backtick_string_runs_over_lines_in_typescript() {
        let code = code_only("x = `a\nb`; y", "typescript");
        assert_eq!(code, "x =   \n  ; y");
    }

    #[test]
    fn a_glob_import_brings_every_variant_and_a_list_only_the_listed_ones() {
        assert_eq!(
            imported_variants("use crate::shapes::Kind::*;", "Kind", &variants()),
            variants()
        );
        assert_eq!(
            imported_variants(
                "pub use super::Kind::{Circle, Square as S};",
                "Kind",
                &variants()
            ),
            ["Circle"]
        );
        assert_eq!(
            imported_variants(
                "use a::{Kind::{self, Square}, b::Circle};",
                "Kind",
                &variants()
            ),
            ["Square"]
        );
        assert!(imported_variants("use a::Other::*;", "Kind", &variants()).is_empty());
        assert!(imported_variants("use a::Kind;", "Kind", &variants()).is_empty());
    }

    #[test]
    fn a_case_label_is_found_before_a_name_and_before_a_name_in_a_list() {
        assert!(follows_case("        case "));
        assert!(follows_case("        case CIRCLE, "));
        assert!(!follows_case("        return "));
        assert!(!follows_case("        showcase "));
        assert!(!follows_case("        x = f(a, "));
    }

    #[test]
    fn bare_names_skip_paths_calls_declarations_and_use_items() {
        let names: BTreeSet<&str> = ["Circle", "Square"].into_iter().collect();
        let code = "use a::{Circle};\nlet x = Kind::Circle;\nlet y = o.Square;\nstruct Circle;\nmatch k {\n  Square => 1,\n}";
        let found = bare_names(code, &names);
        assert_eq!(found, BTreeMap::from([("Square".to_string(), 5)]));
    }
}
