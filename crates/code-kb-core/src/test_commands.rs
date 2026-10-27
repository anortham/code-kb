//! The shell command that runs one test, from the marker files of the project.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A class that encloses a test. `test_case` says it derives from `unittest.TestCase`.
pub struct TestClass<'a> {
    pub name: &'a str,
    pub test_case: bool,
}

/// A test whose command is wanted. `path` is relative to the project root and uses `/`. `kind` is
/// the symbol kind (`function` or `method`) and `line` its first line. `classes` are the classes
/// that enclose the test, outermost first.
pub struct TestSpec<'a> {
    pub language: &'a str,
    pub kind: &'a str,
    pub path: &'a str,
    pub line: usize,
    pub name: &'a str,
    pub classes: &'a [TestClass<'a>],
}

/// Finds test commands. Runner markers and pytest configuration are read at most once per folder
/// for the life of the finder; project-file entries for .NET are listed once. A Rust or Go test
/// file is read at most once for the same time.
pub struct CommandFinder {
    root: PathBuf,
    folders: HashMap<String, Folder>,
    sources: HashMap<String, Vec<String>>,
}

#[derive(Clone, Copy)]
enum Runner {
    Vitest,
    Jest,
    Mocha,
    NodeTest,
}

#[derive(Clone, Copy)]
enum Build {
    Maven,
    Gradle,
}

#[derive(Clone)]
struct Cargo {
    package: Option<String>,
    workspace: bool,
}

#[derive(Clone)]
struct PackageJson {
    runner: Option<Runner>,
    has_test_script: bool,
}

struct Folder {
    cargo: Option<Cargo>,
    go_mod: bool,
    package_json: Option<PackageJson>,
    build: Option<Build>,
    gradlew: bool,
    gradlew_bat: bool,
    pytest_config: bool,
    dotnet_test_scope: bool,
}

impl Folder {
    fn read(dir: &Path) -> Self {
        let text = |name: &str| {
            fs::read(dir.join(name))
                .ok()
                .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        };
        let has = |name: &str| dir.join(name).is_file();
        let pyproject = text("pyproject.toml");
        let setup_cfg = text("setup.cfg");
        let tox_ini = text("tox.ini");
        let pytest_config = has("pytest.ini")
            || pyproject.as_deref().is_some_and(|content| {
                content
                    .lines()
                    .any(|line| line.trim() == "[tool.pytest.ini_options]")
            })
            || setup_cfg
                .as_deref()
                .is_some_and(|content| content.lines().any(|line| line.trim() == "[tool:pytest]"))
            || tox_ini
                .as_deref()
                .is_some_and(|content| content.lines().any(|line| line.trim() == "[pytest]"));
        Folder {
            cargo: text("Cargo.toml").map(|toml| Cargo {
                package: package_name(&toml),
                workspace: has_workspace(&toml),
            }),
            go_mod: has("go.mod"),
            package_json: text("package.json").map(|json| PackageJson {
                runner: runner(&json),
                has_test_script: has_test_script(&json),
            }),
            build: if has("pom.xml") {
                Some(Build::Maven)
            } else if has("build.gradle") || has("build.gradle.kts") {
                Some(Build::Gradle)
            } else {
                None
            },
            gradlew: has("gradlew"),
            gradlew_bat: has("gradlew.bat"),
            pytest_config,
            dotnet_test_scope: fs::read_dir(dir)
                .ok()
                .into_iter()
                .flatten()
                .filter_map(Result::ok)
                .any(|entry| {
                    let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
                    if is_dotnet_solution_marker(&name) {
                        true
                    } else if is_dotnet_project_marker(&name) {
                        fs::read_to_string(entry.path())
                            .is_ok_and(|source| is_dotnet_test_project(&source))
                    } else {
                        false
                    }
                }),
        }
    }
}

/// The `name` key of the `[package]` table.
pub(crate) fn package_name(toml: &str) -> Option<String> {
    let mut in_package = false;
    for line in toml.lines().map(str::trim) {
        if line.starts_with('[') {
            in_package = line == "[package]";
        } else if in_package
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == "name"
        {
            return value.split(['"', '\'']).nth(1).map(str::to_string);
        }
    }
    None
}

fn has_workspace(toml: &str) -> bool {
    toml.lines()
        .map(str::trim)
        .any(|line| line == "[workspace]" || line.starts_with("[workspace."))
}

fn is_dotnet_solution_marker(name: &str) -> bool {
    [".sln", ".slnx"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn is_dotnet_project_marker(name: &str) -> bool {
    [".csproj", ".fsproj", ".vbproj"]
        .iter()
        .any(|extension| name.ends_with(extension))
}

fn is_dotnet_test_project(source: &str) -> bool {
    let normalized = source
        .chars()
        .filter(|character| !character.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized.contains("<istestproject>false</istestproject>") {
        return false;
    }
    normalized.contains("<istestproject>true</istestproject>")
        || normalized.contains("microsoft.net.test.sdk")
}

fn gradle_test_command(windows: bool, gradlew: bool, gradlew_bat: bool) -> String {
    if windows && gradlew_bat {
        "gradlew.bat test".to_string()
    } else if gradlew {
        "./gradlew test".to_string()
    } else {
        "gradle test".to_string()
    }
}

fn runner(package_json: &str) -> Option<Runner> {
    let json: serde_json::Value = serde_json::from_str(package_json).ok()?;
    let script = json["scripts"]["test"].as_str().unwrap_or_default();
    let uses = |name: &str| {
        ["devDependencies", "dependencies"]
            .iter()
            .any(|section| json[*section].get(name).is_some())
            || script.split_whitespace().any(|word| word == name)
    };
    if uses("vitest") {
        Some(Runner::Vitest)
    } else if uses("jest") {
        Some(Runner::Jest)
    } else if uses("mocha") {
        Some(Runner::Mocha)
    } else if script.contains("node --test") {
        Some(Runner::NodeTest)
    } else {
        None
    }
}

fn has_test_script(package_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(package_json)
        .ok()
        .and_then(|json| {
            json["scripts"]["test"]
                .as_str()
                .map(str::trim)
                .map(str::to_string)
        })
        .is_some_and(|script| !script.is_empty())
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

fn relative<'a>(dir: &str, path: &'a str) -> &'a str {
    if dir.is_empty() {
        path
    } else {
        path.strip_prefix(dir)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(path)
    }
}

fn backslashed(name: &str, special: impl Fn(char) -> bool) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if special(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `name` made safe inside a double-quoted shell word.
fn double_quoted(name: &str) -> String {
    backslashed(name, |c| matches!(c, '"' | '\\' | '$' | '`'))
}

fn single_quoted(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// `text` as one shell word: unchanged when it holds only `A-Z a-z 0-9 _ . / -`, else
/// single-quoted.
fn shell_word(text: &str) -> String {
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'));
    if plain {
        text.to_string()
    } else {
        single_quoted(text)
    }
}

/// `command` run from the folder `dir` below the project root.
fn in_folder(dir: &str, command: String) -> String {
    if dir.is_empty() {
        command
    } else {
        format!("cd {} && {command}", shell_word(dir))
    }
}

/// Stands for a character that a command line cannot carry: NUL, or half of a surrogate pair.
const UNREPRESENTABLE: char = '\0';

/// A JavaScript test name as the string its source literal holds. julie gives the literal text, so
/// `\'` becomes `'`, `\\` becomes `\`, and `\n`, `\x41`, `\u0041`, `\u{41}`, and `\101` become
/// the character they name. A backslash before a line break is a line continuation: both vanish.
/// A character a command line cannot carry becomes NUL.
fn js_unescaped(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        rest = &rest[c.len_utf8()..];
        if c != '\\' {
            out.push(c);
            continue;
        }
        let Some(escaped) = rest.chars().next() else {
            out.push('\\');
            break;
        };
        if let Some(len) = line_break_len(rest) {
            rest = &rest[len..];
            continue;
        }
        let (decoded, len) = decoded_escape(rest).unwrap_or((escaped, escaped.len_utf8()));
        out.push(decoded);
        rest = &rest[len..];
    }
    out
}

/// The length of the line break that starts `text`: `\n`, `\r\n`, `\r`, U+2028, or U+2029.
fn line_break_len(text: &str) -> Option<usize> {
    if text.starts_with("\r\n") {
        return Some(2);
    }
    text.chars()
        .next()
        .filter(|c| matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
        .map(char::len_utf8)
}

/// The character that the escape after a backslash names, with the length of the escape in bytes.
/// `None` for an escape that names itself or is malformed.
fn decoded_escape(text: &str) -> Option<(char, usize)> {
    match text.as_bytes().first()? {
        b'n' => Some(('\n', 1)),
        b't' => Some(('\t', 1)),
        b'r' => Some(('\r', 1)),
        b'b' => Some(('\u{8}', 1)),
        b'f' => Some(('\u{c}', 1)),
        b'v' => Some(('\u{b}', 1)),
        b'0'..=b'3' => octal_escape(text, 3),
        b'4'..=b'7' => octal_escape(text, 2),
        b'x' => Some((char::from_u32(hex_value(text.get(1..3)?)?)?, 3)),
        b'u' => unicode_escape(text),
        _ => None,
    }
}

fn octal_escape(text: &str, max_digits: usize) -> Option<(char, usize)> {
    let len = text
        .bytes()
        .take(max_digits)
        .take_while(|byte| matches!(byte, b'0'..=b'7'))
        .count();
    Some((
        char::from_u32(u32::from_str_radix(&text[..len], 8).ok()?)?,
        len,
    ))
}

fn hex_value(digits: &str) -> Option<u32> {
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(digits, 16).ok()
}

/// `text` starts with the `u` of `\uHHHH` or `\u{H…}`. A pair of `\u` surrogates is one character.
fn unicode_escape(text: &str) -> Option<(char, usize)> {
    if let Some(braced) = text.strip_prefix("u{") {
        let digits = &braced[..braced.find('}')?];
        let code = hex_value(digits).filter(|code| *code <= 0x10FFFF)?;
        let character = char::from_u32(code).unwrap_or(UNREPRESENTABLE);
        return Some((character, digits.len() + 3));
    }
    let high = hex_value(text.get(1..5)?)?;
    let low = text
        .get(5..)
        .and_then(|after| after.strip_prefix("\\u"))
        .and_then(|after| hex_value(after.get(..4)?))
        .filter(|low| (0xDC00..=0xDFFF).contains(low));
    match (high, low) {
        (0xD800..=0xDBFF, Some(low)) => {
            let code = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
            Some((char::from_u32(code)?, 11))
        }
        _ => Some((char::from_u32(high).unwrap_or(UNREPRESENTABLE), 5)),
    }
}

/// The length of the `${…}` hole of a template literal that starts `text`, up to its matching `}`.
/// A string or a template inside the hole may hold a `}`.
fn hole_len(text: &str) -> Option<usize> {
    let code = text.strip_prefix("${")?;
    Some(2 + closing_brace(code.as_bytes())? + 1)
}

/// The index of the `}` that closes the code in `code`, which starts after a `{`.
fn closing_brace(code: &[u8]) -> Option<usize> {
    let mut depth = 0;
    let mut at = 0;
    while at < code.len() {
        match code[at] {
            b'{' => depth += 1,
            b'}' if depth == 0 => return Some(at),
            b'}' => depth -= 1,
            b'\'' | b'"' | b'`' => {
                at = after_literal(code, at)?;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    None
}

/// The index after the string or template literal that starts at `start`. A template may hold
/// holes of its own.
fn after_literal(code: &[u8], start: usize) -> Option<usize> {
    let quote = code[start];
    let mut at = start + 1;
    while at < code.len() {
        match code[at] {
            b'\\' => at += 1,
            b'$' if quote == b'`' && code.get(at + 1) == Some(&b'{') => {
                at += 2 + closing_brace(&code[at + 2..])?;
            }
            byte if byte == quote => return Some(at + 1),
            _ => {}
        }
        at += 1;
    }
    None
}

/// The text of a JavaScript test name between its `${…}` holes, decoded by `js_unescaped`. A
/// character a command line cannot carry ends a part like a hole does.
fn literal_parts(name: &str) -> Vec<String> {
    let bytes = name.as_bytes();
    let mut raw_parts = Vec::new();
    let (mut start, mut at) = (0, 0);
    while at < bytes.len() {
        if bytes[at] == b'\\' {
            at += 2;
        } else if bytes[at] == b'$'
            && let Some(len) = hole_len(&name[at..])
        {
            raw_parts.push(&name[start..at]);
            at += len;
            start = at;
        } else {
            at += 1;
        }
    }
    raw_parts.push(&name[start..]);
    raw_parts
        .into_iter()
        .flat_map(|raw| {
            js_unescaped(raw)
                .split(UNREPRESENTABLE)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect()
}

fn push_regex_escaped(out: &mut String, c: char) {
    match c {
        '\n' => out.push_str("\\n"),
        '\t' => out.push_str("\\t"),
        '\r' => out.push_str("\\r"),
        '\u{2028}' | '\u{2029}' => out.push_str(&format!("\\u{:04x}", u32::from(c))),
        c if c.is_control() => out.push_str(&format!("\\x{:02x}", u32::from(c))),
        '\\' | '^' | '$' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' => {
            out.push('\\');
            out.push(c);
        }
        _ => out.push(c),
    }
}

fn is_word_byte(byte: &u8) -> bool {
    byte.is_ascii_alphanumeric() || *byte == b'_'
}

/// The length of the `test.each` placeholder that starts `text`: `%s %d %i %f %j %o %O %p %#`, or
/// `$name` with optional `.path` segments.
fn placeholder_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    match bytes {
        [
            b'%',
            b's' | b'd' | b'i' | b'f' | b'j' | b'o' | b'O' | b'p' | b'#',
            ..,
        ] => Some(2),
        [b'$', first, ..] if first.is_ascii_alphabetic() || *first == b'_' => {
            let mut len = 1;
            loop {
                len += bytes[len..].iter().take_while(|b| is_word_byte(b)).count();
                match &bytes[len..] {
                    [b'.', next, ..] if is_word_byte(next) => len += 1,
                    _ => return Some(len),
                }
            }
        }
        _ => None,
    }
}

/// `name` as the regular expression of `-t`: each placeholder matches any text, `%%` matches one
/// or two percent signs (`test.each` writes one, a plain test writes two), and every other
/// character matches itself.
fn each_pattern(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut rest = name;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("%%") {
            out.push_str("%%?");
            rest = &rest[2..];
        } else if let Some(len) = placeholder_len(rest) {
            out.push_str(".*");
            rest = &rest[len..];
        } else {
            push_regex_escaped(&mut out, c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The regular expression of `-t` for a name: each hole between its literal parts matches any
/// text.
fn name_pattern(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| each_pattern(part))
        .collect::<Vec<_>>()
        .join(".*")
}

/// A character that would split the command line: a control character other than a tab, or a
/// Unicode line or paragraph separator.
fn splits_a_line(c: char) -> bool {
    (c.is_control() && c != '\t') || matches!(c, '\u{2028}' | '\u{2029}')
}

/// The part of a name that `--fgrep` can match as a substring: the longest piece, between holes
/// and characters that split a line, that holds more than blanks. The first of equal pieces wins.
fn longest_literal(parts: &[String]) -> Option<&str> {
    parts
        .iter()
        .flat_map(|part| part.split(splits_a_line))
        .filter(|piece| !piece.trim().is_empty())
        .rev()
        .max_by_key(|piece| piece.len())
}

fn depth(dir: &str) -> usize {
    if dir.is_empty() {
        0
    } else {
        dir.split('/').count()
    }
}

/// The path of the Gradle wrapper in `wrapper_dir` as seen from `dir`, a folder at or below it.
fn wrapper_path(dir: &str, wrapper_dir: &str) -> String {
    match depth(dir) - depth(wrapper_dir) {
        0 => "./gradlew".to_string(),
        up => format!("{}gradlew", "../".repeat(up)),
    }
}

/// Whether `go test` reads `name` as a function of the kind `prefix`: the prefix alone, or the
/// prefix followed by anything but a lowercase letter.
fn names_go_test(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| !rest.chars().next().is_some_and(char::is_lowercase))
}

/// Whether pytest collects a test in these classes under its defaults: every class is named
/// `Test…` or derives from `unittest.TestCase`, and no class around another is a `TestCase`,
/// because pytest collects only the methods of one.
fn pytest_collects(classes: &[TestClass]) -> bool {
    let named = classes
        .iter()
        .all(|class| class.test_case || class.name.starts_with("Test"));
    let inside_test_case = classes
        .split_last()
        .is_some_and(|(_, outer)| outer.iter().any(|class| class.test_case));
    named && !inside_test_case
}

/// `pytest path::Class::name`. pytest rejects a node id whose path holds `[` and splits one at
/// `::`, so a test in such a folder runs from that folder; a file name with either has no command.
fn python(test: &TestSpec) -> Option<String> {
    if !pytest_collects(test.classes) {
        return None;
    }
    let breaks_a_node_id = |text: &str| text.contains('[') || text.contains("::");
    let (dir, file) = test.path.rsplit_once('/').unwrap_or(("", test.path));
    if breaks_a_node_id(file) {
        return None;
    }
    let (dir, file) = if breaks_a_node_id(dir) {
        (dir, file)
    } else {
        ("", test.path)
    };
    let classes: String = test
        .classes
        .iter()
        .map(|class| format!("{}::", class.name))
        .collect();
    Some(in_folder(
        dir,
        format!("pytest {}::{classes}{}", shell_word(file), test.name),
    ))
}

impl CommandFinder {
    /// A finder for the project at `root`. It never looks above `root`.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
            folders: HashMap::new(),
            sources: HashMap::new(),
        }
    }

    /// The command that runs `test`, or `None` when the language or the runner is unknown.
    pub fn command(&mut self, test: &TestSpec) -> Option<String> {
        match test.language {
            "python" => python(test),
            "rust" => Some(self.rust(test)),
            "go" => self.go(test),
            "javascript" | "typescript" | "tsx" | "jsx" => self.script(test),
            "java" => self.java(test),
            "csharp" => test.classes.last().map(|class| {
                let filter = double_quoted(&format!("{}.{}", class.name, test.name));
                format!("dotnet test --filter \"FullyQualifiedName~{filter}\"")
            }),
            _ => None,
        }
    }

    /// The configured command for the broader suite nearest to `path`.
    pub fn broader_test_command(&mut self, path: Option<&str>) -> Option<String> {
        let directory = path.map(parent_dir).unwrap_or("");
        self.nearest(directory, |folder| {
            if let Some(cargo) = &folder.cargo {
                Some(if cargo.workspace {
                    "cargo test --workspace".to_string()
                } else {
                    "cargo test".to_string()
                })
            } else if folder.go_mod {
                Some("go test ./...".to_string())
            } else if folder
                .package_json
                .as_ref()
                .is_some_and(|package| package.has_test_script)
            {
                Some("npm test".to_string())
            } else if matches!(folder.build, Some(Build::Maven)) {
                Some("mvn test".to_string())
            } else if matches!(folder.build, Some(Build::Gradle)) {
                Some(gradle_test_command(
                    cfg!(windows),
                    folder.gradlew,
                    folder.gradlew_bat,
                ))
            } else if folder.pytest_config {
                Some("pytest".to_string())
            } else if folder.dotnet_test_scope {
                Some("dotnet test".to_string())
            } else {
                None
            }
        })
        .map(|(directory, command)| in_folder(&directory, command))
    }

    fn folder(&mut self, dir: &str) -> &Folder {
        let root = &self.root;
        self.folders
            .entry(dir.to_string())
            .or_insert_with(|| Folder::read(&root.join(dir)))
    }

    /// The folder nearest to `dir`, at or above it and not above the root, that `pick` accepts,
    /// with what `pick` found there.
    fn nearest<T>(
        &mut self,
        dir: &str,
        pick: impl Fn(&Folder) -> Option<T>,
    ) -> Option<(String, T)> {
        let mut dir = dir;
        loop {
            if let Some(found) = pick(self.folder(dir)) {
                return Some((dir.to_string(), found));
            }
            if dir.is_empty() {
                return None;
            }
            dir = parent_dir(dir);
        }
    }

    /// The lines of the file `path`, empty when it cannot be read.
    fn source(&mut self, path: &str) -> &[String] {
        let root = &self.root;
        self.sources.entry(path.to_string()).or_insert_with(|| {
            fs::read_to_string(root.join(path))
                .map(|text| text.lines().map(str::to_string).collect())
                .unwrap_or_default()
        })
    }

    fn rust(&mut self, test: &TestSpec) -> String {
        let ignored = if self.has_ignore_attribute(test) {
            " -- --ignored"
        } else {
            ""
        };
        let name = format!("{}{ignored}", test.name);
        let Some((dir, cargo)) = self.nearest(parent_dir(test.path), |folder| folder.cargo.clone())
        else {
            return format!("cargo test {name}");
        };
        let command = match cargo.package {
            None => format!("cargo test {name}"),
            Some(package) => match relative(&dir, test.path)
                .strip_prefix("tests/")
                .and_then(|rest| rest.strip_suffix(".rs"))
                .filter(|stem| !stem.contains('/'))
            {
                Some(stem) => {
                    format!("cargo test -p {package} --test {} {name}", shell_word(stem))
                }
                None => format!("cargo test -p {package} {name}"),
            },
        };
        if self
            .folder("")
            .cargo
            .as_ref()
            .is_some_and(|root| root.workspace)
        {
            command
        } else {
            in_folder(&dir, command)
        }
    }

    /// Whether an attribute above the function says `#[ignore]` or `#[ignore = "…"]`.
    fn has_ignore_attribute(&mut self, test: &TestSpec) -> bool {
        let above = test.line.saturating_sub(1);
        self.source(test.path)
            .iter()
            .take(above)
            .rev()
            .map(|line| line.trim())
            .take_while(|line| line.is_empty() || line.starts_with('#') || line.starts_with("//"))
            .filter(|line| line.starts_with('#'))
            .any(|line| {
                line.match_indices("#[ignore").any(|(at, found)| {
                    matches!(
                        line[at + found.len()..].trim_start().chars().next(),
                        Some(']' | '=')
                    )
                })
            })
    }

    /// `None` for a function that `go test` does not run by name: a method, a name that is not
    /// `Test`, `Benchmark`, `Fuzz`, or `Example` followed by a capital, `TestMain`, a file that is
    /// not `_test.go`, or an example with no `// Output:` comment.
    fn go(&mut self, test: &TestSpec) -> Option<String> {
        let name = test.name;
        if test.kind != "function" || !test.path.ends_with("_test.go") {
            return None;
        }
        let selector = if names_go_test(name, "Benchmark") {
            format!("-bench '^{name}$' -run '^$'")
        } else if (names_go_test(name, "Test") && name != "TestMain")
            || names_go_test(name, "Fuzz")
            || (names_go_test(name, "Example") && self.go_example_prints_output(test))
        {
            format!("-run '^{name}$'")
        } else {
            return None;
        };
        let module = self
            .nearest(parent_dir(test.path), |folder| folder.go_mod.then_some(()))
            .map_or_else(String::new, |(dir, _)| dir);
        let dir = match parent_dir(relative(&module, test.path)) {
            "" => ".",
            dir => dir,
        };
        Some(in_folder(
            &module,
            format!("go test {} {selector}", shell_word(&format!("./{dir}"))),
        ))
    }

    /// Whether the example that starts on `test.line` ends in an `// Output:` comment. `go test`
    /// compiles an example without one and does not run it. The body ends at the first `}` in
    /// column 0, which `gofmt` guarantees.
    fn go_example_prints_output(&mut self, test: &TestSpec) -> bool {
        let mut body = self
            .source(test.path)
            .iter()
            .skip(test.line.saturating_sub(1));
        if body
            .next()
            .is_none_or(|first| first.trim_end().ends_with('}'))
        {
            return false;
        }
        body.take_while(|line| line.as_str() != "}").any(|line| {
            let comment = line.trim().strip_prefix("//").unwrap_or("").trim_start();
            let comment = comment.to_ascii_lowercase();
            comment.starts_with("output:") || comment.starts_with("unordered output:")
        })
    }

    fn script(&mut self, test: &TestSpec) -> Option<String> {
        let (dir, package) =
            self.nearest(parent_dir(test.path), |folder| folder.package_json.clone())?;
        let path = shell_word(relative(&dir, test.path));
        let parts = literal_parts(test.name);
        let pattern = || double_quoted(&name_pattern(&parts));
        let command = match package.runner? {
            Runner::Vitest => format!("npx vitest run {path} -t \"{}\"", pattern()),
            Runner::Jest => format!("npx jest --runTestsByPath {path} -t \"{}\"", pattern()),
            Runner::Mocha => match longest_literal(&parts) {
                Some(part) => format!("npx mocha {path} --fgrep \"{}\"", double_quoted(part)),
                None => format!("npx mocha {path}"),
            },
            Runner::NodeTest => format!(
                "node --test-name-pattern \"{}\" {path}",
                double_quoted(&format!("^{}$", name_pattern(&parts)))
            ),
        };
        Some(in_folder(&dir, command))
    }

    fn java(&mut self, test: &TestSpec) -> Option<String> {
        if test.classes.is_empty() {
            return None;
        }
        let class = test
            .classes
            .iter()
            .map(|class| class.name)
            .collect::<Vec<_>>()
            .join("$");
        let (dir, build) = self.nearest(parent_dir(test.path), |folder| folder.build)?;
        let command = match build {
            Build::Maven => format!(
                "mvn test -Dtest={}#{}",
                shell_word(&class),
                shell_word(test.name)
            ),
            Build::Gradle => {
                let tool = match self.nearest(&dir, |folder| folder.gradlew.then_some(())) {
                    Some((wrapper_dir, _)) => wrapper_path(&dir, &wrapper_dir),
                    None => "gradle".to_string(),
                };
                let filter = single_quoted(&format!("{class}.{}", test.name));
                format!("{tool} test --tests {filter}")
            }
        };
        Some(in_folder(&dir, command))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safe_tempdir;
    use std::fs;

    fn project(files: &[(&str, &str)]) -> tempfile::TempDir {
        let temp = safe_tempdir();
        for (path, content) in files {
            let full = temp.path().join(path);
            fs::create_dir_all(full.parent().unwrap()).unwrap();
            fs::write(full, content).unwrap();
        }
        temp
    }

    fn command(
        project: &tempfile::TempDir,
        language: &str,
        path: &str,
        name: &str,
        class: Option<&str>,
    ) -> Option<String> {
        nested(project, language, path, name, class.as_slice())
    }

    fn script(files: &[(&str, &str)], path: &str, name: &str) -> Option<String> {
        command(&project(files), "typescript", path, name, None)
    }

    fn nested(
        project: &tempfile::TempDir,
        language: &str,
        path: &str,
        name: &str,
        classes: &[&str],
    ) -> Option<String> {
        let classes: Vec<TestClass> = classes
            .iter()
            .map(|name| TestClass {
                name,
                test_case: false,
            })
            .collect();
        CommandFinder::new(project.path()).command(&TestSpec {
            language,
            kind: "function",
            path,
            line: 1,
            name,
            classes: &classes,
        })
    }

    #[test]
    fn python_tests_run_through_pytest_with_the_class_only_for_a_member_of_one() {
        let empty = project(&[]);

        assert_eq!(
            command(&empty, "python", "tests/test_a.py", "test_one", None).as_deref(),
            Some("pytest tests/test_a.py::test_one")
        );
        assert_eq!(
            command(
                &empty,
                "python",
                "tests/test_a.py",
                "test_one",
                Some("TestA")
            )
            .as_deref(),
            Some("pytest tests/test_a.py::TestA::test_one")
        );
    }

    #[test]
    fn a_rust_test_names_the_package_of_the_nearest_cargo_toml() {
        let crate_dir = project(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/a\"]\n\n[package]\nname = \"outer\"\n",
            ),
            (
                "crates/a/Cargo.toml",
                "[dependencies]\nname = \"not-this\"\n\n[package]\nversion = \"0.1.0\"\nname = \"inner-crate\" # the crate\n\n[lib]\nname = \"also_not\"\n",
            ),
        ]);

        let run = |path: &str| command(&crate_dir, "rust", path, "case", None);

        assert_eq!(
            run("crates/a/src/lib.rs").as_deref(),
            Some("cargo test -p inner-crate case")
        );
        assert_eq!(
            run("crates/a/tests/it.rs").as_deref(),
            Some("cargo test -p inner-crate --test it case")
        );
        assert_eq!(
            run("crates/a/tests/sub/it.rs").as_deref(),
            Some("cargo test -p inner-crate case")
        );
        assert_eq!(
            run("src/main.rs").as_deref(),
            Some("cargo test -p outer case")
        );
    }

    #[test]
    fn a_rust_test_without_a_package_name_runs_by_name_alone() {
        let none = project(&[]);
        let workspace_only = project(&[("Cargo.toml", "[workspace]\nmembers = []\n")]);

        assert_eq!(
            command(&none, "rust", "tests/it.rs", "case", None).as_deref(),
            Some("cargo test case")
        );
        assert_eq!(
            command(&workspace_only, "rust", "src/lib.rs", "case", None).as_deref(),
            Some("cargo test case")
        );
    }

    #[test]
    fn a_go_test_names_its_folder_relative_to_the_nearest_go_mod() {
        let module = project(&[
            ("go.mod", "module example.com/m\n"),
            ("sub/go.mod", "module example.com/m/sub\n"),
        ]);

        let run = |path: &str| command(&module, "go", path, "TestRun", None);

        assert_eq!(
            run("pkg/run_test.go").as_deref(),
            Some("go test ./pkg -run '^TestRun$'")
        );
        assert_eq!(
            run("run_test.go").as_deref(),
            Some("go test ./. -run '^TestRun$'")
        );
        assert_eq!(
            run("sub/deep/pkg/run_test.go").as_deref(),
            Some("cd sub && go test ./deep/pkg -run '^TestRun$'")
        );
        assert_eq!(
            run("sub/run_test.go").as_deref(),
            Some("cd sub && go test ./. -run '^TestRun$'")
        );
    }

    #[test]
    fn a_script_test_picks_the_runner_the_nearest_package_json_names() {
        let run = |body: &str| {
            script(
                &[("package.json", body)],
                "src/a.test.ts",
                "adds \"two\" numbers",
            )
        };

        assert_eq!(
            run(r#"{"devDependencies":{"vitest":"^1"}}"#).as_deref(),
            Some(r#"npx vitest run src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"dependencies":{"jest":"^29"}}"#).as_deref(),
            Some(r#"npx jest --runTestsByPath src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"mocha":"^10"}}"#).as_deref(),
            Some(r#"npx mocha src/a.test.ts --fgrep "adds \"two\" numbers""#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"node --test"}}"#).as_deref(),
            Some(r#"node --test-name-pattern "^adds \"two\" numbers\$" src/a.test.ts"#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"cross-env CI=1 jest --runInBand"}}"#).as_deref(),
            Some(r#"npx jest --runTestsByPath src/a.test.ts -t "adds \"two\" numbers""#)
        );
        assert_eq!(run(r#"{"scripts":{"test":"tap"}}"#), None);
        assert_eq!(run("not json"), None);
    }

    #[test]
    fn a_script_test_uses_the_nearest_package_json_only() {
        let files = [
            ("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#),
            ("pkg/package.json", r#"{"type":"module"}"#),
        ];

        assert_eq!(script(&files, "pkg/a.test.ts", "case"), None);
        assert_eq!(
            script(&files, "other/a.test.ts", "case").as_deref(),
            Some(r#"npx vitest run other/a.test.ts -t "case""#)
        );
        assert_eq!(script(&[], "a.test.ts", "case"), None);
    }

    #[test]
    fn a_script_name_escapes_the_characters_a_double_quoted_shell_word_expands() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", r#"costs $5 `now` a\\b"#).as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "costs \$5 \`now\` a\\b""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_escapes_the_characters_of_a_regular_expression() {
        let name = "should not render evil javascript links in anchor href (prevent XSS)";
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", name).as_deref(),
            Some(
                r#"npx vitest run a.test.ts -t "should not render evil javascript links in anchor href \\(prevent XSS\\)""#
            )
        );
        assert_eq!(
            script(&jest, "a.test.ts", "parses v1.2 (beta)").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t "parses v1\\.2 \\(beta\\)""#)
        );
    }

    #[test]
    fn a_vitest_name_escapes_every_regular_expression_character_before_the_shell_quoting() {
        let files = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", r#"\\ ^ $ . * + ? ( ) [ ] { } | "x""#).as_deref(),
            Some(
                r#"npx vitest run a.test.ts -t "\\\\ \\^ \\\$ \\. \\* \\+ \\? \\( \\) \\[ \\] \\{ \\} \\| \"x\"""#
            )
        );
    }

    #[test]
    fn a_mocha_name_is_a_plain_substring_so_it_keeps_its_regular_expression_characters() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "parses v1.2 (beta)").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "parses v1.2 (beta)""#)
        );
    }

    #[test]
    fn a_script_name_decodes_its_source_escapes_before_any_escaping() {
        let run = |dependency: &str, name: &str| {
            script(
                &[(
                    "package.json",
                    &format!(r#"{{"devDependencies":{{"{dependency}":"^1"}}}}"#),
                )],
                "a.test.js",
                name,
            )
        };

        assert_eq!(
            run("vitest", r"doesn\'t throw on empty input").as_deref(),
            Some(r#"npx vitest run a.test.js -t "doesn't throw on empty input""#)
        );
        assert_eq!(
            run("jest", r#"says \"hi\" politely"#).as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.js -t "says \"hi\" politely""#)
        );
        assert_eq!(
            run("vitest", r"tick \` and back\\slash").as_deref(),
            Some(r#"npx vitest run a.test.js -t "tick \` and back\\\\slash""#)
        );
        assert_eq!(
            run("vitest", r"two\nlines\tand\rmore").as_deref(),
            Some(r#"npx vitest run a.test.js -t "two\\nlines\\tand\\rmore""#)
        );
        assert_eq!(
            run("mocha", r#"doesn\'t say \"hi\" or back\\slash"#).as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "doesn't say \"hi\" or back\\slash""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_turns_each_format_placeholder_into_a_wildcard() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(
                &vitest,
                "a.test.ts",
                "adds %i + %d = %s, %f %j %o %O %p #%#"
            )
            .as_deref(),
            Some(r#"npx vitest run a.test.ts -t "adds .* \\+ .* = .*, .* .* .* .* .* #.*""#)
        );
        assert_eq!(
            script(
                &jest,
                "a.test.ts",
                "$a is $b.c.d. costs $5 or 100% and 50%x"
            )
            .as_deref(),
            Some(
                r#"npx jest --runTestsByPath a.test.ts -t ".* is .*\\. costs \\\$5 or 100% and 50%x""#
            )
        );
    }

    #[test]
    fn a_mocha_name_keeps_its_format_placeholders() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "adds %i + %s for $name").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "adds %i + %s for \$name""#)
        );
    }

    #[test]
    fn a_mocha_name_keeps_a_double_percent_as_written() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&files, "a.test.ts", "pct 100%% sure").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "pct 100%% sure""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_matches_a_double_percent_as_one_or_two_percent_signs() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", "pct 100%% sure %s").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "pct 100%%? sure .*""#)
        );
        assert_eq!(
            script(&jest, "a.test.ts", "%%%s%%").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t "%%?.*%%?""#)
        );
    }

    #[test]
    fn a_vitest_or_jest_name_turns_each_template_hole_into_a_wildcard() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.ts", "handles ${kind} kind").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "handles .* kind""#)
        );
        assert_eq!(
            script(&jest, "a.test.ts", "${a} and ${b.c}! (${ {x: 1}.x })").as_deref(),
            Some(r#"npx jest --runTestsByPath a.test.ts -t ".* and .*! \\(.*\\)""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", "costs ${5").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "costs \\\$\\{5""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", r"costs \${5} total").as_deref(),
            Some(r#"npx vitest run a.test.ts -t "costs \\\$\\{5\\} total""#)
        );
        assert_eq!(
            script(&vitest, "a.test.ts", "$a and ${b}$c").as_deref(),
            Some(r#"npx vitest run a.test.ts -t ".* and .*.*""#)
        );
    }

    #[test]
    fn a_mocha_name_with_a_template_hole_uses_its_longest_literal_part() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&files, "a.test.ts", name);

        assert_eq!(
            run("handles ${kind} kind").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "handles ""#)
        );
        assert_eq!(
            run("a ${x} and a longer ${ {y: 1}.y } tail").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep " and a longer ""#)
        );
        assert_eq!(
            run("ab${x}cd").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "ab""#)
        );
        assert_eq!(
            run(r"costs \${5} total").as_deref(),
            Some(r#"npx mocha a.test.ts --fgrep "costs \${5} total""#)
        );
    }

    #[test]
    fn a_mocha_name_with_no_literal_part_has_no_name_filter() {
        let files = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&files, "a.test.ts", name);

        assert_eq!(run("${kind}").as_deref(), Some("npx mocha a.test.ts"));
        assert_eq!(run("${a} ${b}").as_deref(), Some("npx mocha a.test.ts"));
    }

    #[test]
    fn a_script_name_decodes_hex_unicode_and_octal_escapes() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"caf\xe9 é \u{1F600} 😀").as_deref(),
            Some("npx vitest run a.test.js -t \"café é 😀 😀\"")
        );
        assert_eq!(
            script(&mocha, "a.test.js", r"\101\60\8 \x41B \\x41").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "A08 AB \\x41""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"a\bb\fc\vd").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\x08b\\x0cc\\x0bd""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"bad \xZ1 \u12 \u{110000} \u{}").as_deref(),
            Some(r#"npx vitest run a.test.js -t "bad xZ1 u12 u\\{110000\\} u\\{\\}""#)
        );
    }

    #[test]
    fn a_character_a_command_cannot_carry_splits_a_script_name_like_a_template_hole() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"a\0b\x00c\ud83dd").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a.*b.*c.*d""#)
        );
        assert_eq!(
            script(&mocha, "a.test.js", r"a\0bcd\ud83de").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "bcd""#)
        );
    }

    #[test]
    fn a_backslash_before_a_multibyte_character_or_at_the_end_of_a_name_keeps_the_hole_scan_in_step()
     {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", r"caf\é ${x} é${y}").as_deref(),
            Some(r#"npx vitest run a.test.js -t "café .* é.*""#)
        );
        assert_eq!(
            script(&vitest, "a.test.js", r"end\").as_deref(),
            Some(r#"npx vitest run a.test.js -t "end\\\\""#)
        );
    }

    #[test]
    fn a_path_with_a_space_or_shell_character_is_single_quoted_in_every_command() {
        let scripts =
            |dependency: &str| format!(r#"{{"devDependencies":{{"{dependency}":"^1"}}}}"#);
        let tricky = "it's $x/a b.test.ts";
        let quoted = r#"'it'\''s $x/a b.test.ts'"#;
        let run =
            |dependency: &str| script(&[("package.json", &scripts(dependency))], tricky, "case");

        assert_eq!(
            run("vitest").as_deref(),
            Some(format!(r#"npx vitest run {quoted} -t "case""#).as_str())
        );
        assert_eq!(
            run("jest").as_deref(),
            Some(format!(r#"npx jest --runTestsByPath {quoted} -t "case""#).as_str())
        );
        assert_eq!(
            run("mocha").as_deref(),
            Some(format!(r#"npx mocha {quoted} --fgrep "case""#).as_str())
        );
        assert_eq!(
            script(
                &[("package.json", r#"{"scripts":{"test":"node --test"}}"#)],
                tricky,
                "case"
            )
            .as_deref(),
            Some(format!(r#"node --test-name-pattern "^case\$" {quoted}"#).as_str())
        );
    }

    #[test]
    fn a_path_with_a_space_is_single_quoted_for_pytest_cargo_and_go() {
        let empty = project(&[]);
        let cargo = project(&[("Cargo.toml", "[package]\nname = \"p\"\n")]);
        let module = project(&[("go.mod", "module m\n")]);

        assert_eq!(
            command(&empty, "python", "tests/a b.py", "test_one", Some("TestA")).as_deref(),
            Some("pytest 'tests/a b.py'::TestA::test_one")
        );
        assert_eq!(
            command(&empty, "python", "tests/a_b.py", "test_one", None).as_deref(),
            Some("pytest tests/a_b.py::test_one")
        );
        assert_eq!(
            command(&cargo, "rust", "tests/my test.rs", "case", None).as_deref(),
            Some("cargo test -p p --test 'my test' case")
        );
        assert_eq!(
            command(&module, "go", "my pkg/run_test.go", "TestRun", None).as_deref(),
            Some("go test './my pkg' -run '^TestRun$'")
        );
    }

    #[test]
    fn a_jest_command_names_its_file_by_path_so_no_character_is_a_regular_expression() {
        let jest = [("package.json", r#"{"devDependencies":{"jest":"^29"}}"#)];

        for (path, word) in [
            ("test/(grp)/c.test.js", "'test/(grp)/c.test.js'"),
            ("test/[id]/c.test.js", "'test/[id]/c.test.js'"),
            ("test/a+b/c.test.js", "'test/a+b/c.test.js'"),
            ("test/d$x/c.test.js", "'test/d$x/c.test.js'"),
            ("test/a.test.js", "test/a.test.js"),
        ] {
            assert_eq!(
                script(&jest, path, "case").as_deref(),
                Some(format!(r#"npx jest --runTestsByPath {word} -t "case""#).as_str()),
                "{path}"
            );
        }
    }

    #[test]
    fn a_node_test_command_runs_the_file_itself_so_no_character_is_a_glob() {
        let files = [("package.json", r#"{"scripts":{"test":"node --test"}}"#)];

        for (path, word) in [
            ("test/[id]/a.test.js", "'test/[id]/a.test.js'"),
            ("test/+(x)/a.test.js", "'test/+(x)/a.test.js'"),
            ("test/!(x)/a.test.js", "'test/!(x)/a.test.js'"),
            ("test/{a,b}/a.test.js", "'test/{a,b}/a.test.js'"),
            ("test/a.test.js", "test/a.test.js"),
        ] {
            assert_eq!(
                script(&files, path, "case").as_deref(),
                Some(format!(r#"node --test-name-pattern "^case\$" {word}"#).as_str()),
                "{path}"
            );
        }
    }

    #[test]
    fn a_python_test_in_a_nested_class_names_every_enclosing_class() {
        let empty = project(&[]);

        assert_eq!(
            nested(
                &empty,
                "python",
                "tests/test_n.py",
                "test_deep",
                &["TestOuter", "TestInner"]
            )
            .as_deref(),
            Some("pytest tests/test_n.py::TestOuter::TestInner::test_deep")
        );
        assert_eq!(
            nested(
                &empty,
                "python",
                "tests/test_n.py",
                "test_top",
                &["TestOuter"]
            )
            .as_deref(),
            Some("pytest tests/test_n.py::TestOuter::test_top")
        );
    }

    #[test]
    fn a_java_test_in_a_nested_class_names_the_binary_class_name_as_one_shell_word() {
        let maven = project(&[("pom.xml", "<project/>")]);
        let gradle = project(&[("gradlew", ""), ("build.gradle", "")]);
        let path = "src/test/java/OuterTest.java";
        let classes = ["OuterTest", "InnerTest", "DeepTest"];

        assert_eq!(
            nested(&maven, "java", path, "checks", &classes).as_deref(),
            Some(r#"mvn test -Dtest='OuterTest$InnerTest$DeepTest'#checks"#)
        );
        assert_eq!(
            nested(&gradle, "java", path, "checks", &classes).as_deref(),
            Some(r#"./gradlew test --tests 'OuterTest$InnerTest$DeepTest.checks'"#)
        );
        assert_eq!(
            nested(&maven, "java", path, "checks", &["OuterTest"]).as_deref(),
            Some("mvn test -Dtest=OuterTest#checks")
        );
    }

    #[test]
    fn a_java_or_csharp_name_with_a_shell_character_stays_one_shell_word() {
        let maven = project(&[("pom.xml", "<project/>")]);
        let gradle = project(&[("build.gradle", "")]);
        let path = "src/test/java/FooTest.java";

        assert_eq!(
            nested(&maven, "java", path, "a$b `c` it's", &["Foo$1"]).as_deref(),
            Some(r#"mvn test -Dtest='Foo$1'#'a$b `c` it'\''s'"#)
        );
        assert_eq!(
            nested(&gradle, "java", path, "a$b it's", &["Foo$1"]).as_deref(),
            Some(r#"gradle test --tests 'Foo$1.a$b it'\''s'"#)
        );
        assert_eq!(
            nested(&maven, "csharp", "T/FooTests.cs", "C\"D$`e`\\f", &["A$B"]).as_deref(),
            Some(r#"dotnet test --filter "FullyQualifiedName~A\$B.C\"D\$\`e\`\\f""#)
        );
    }

    #[test]
    fn a_csharp_test_in_a_nested_class_names_the_innermost_class() {
        let empty = project(&[]);

        assert_eq!(
            nested(
                &empty,
                "csharp",
                "T/FooTests.cs",
                "Checks",
                &["FooTests", "Inner"]
            )
            .as_deref(),
            Some("dotnet test --filter \"FullyQualifiedName~Inner.Checks\"")
        );
    }

    #[test]
    fn a_backslash_before_a_line_break_in_a_script_name_drops_both() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];

        for name in [
            "part one \\\npart two",
            "part one \\\r\npart two",
            "part one \\\rpart two",
            "part one \\\u{2028}part two",
            "part one \\\u{2029}part two",
        ] {
            assert_eq!(
                script(&vitest, "a.test.js", name).as_deref(),
                Some(r#"npx vitest run a.test.js -t "part one part two""#),
                "{name:?}"
            );
            assert_eq!(
                script(&mocha, "a.test.js", name).as_deref(),
                Some(r#"npx mocha a.test.js --fgrep "part one part two""#),
                "{name:?}"
            );
        }
        assert_eq!(
            script(&vitest, "a.test.js", "a\\\\\nb").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\\\\\nb""#)
        );
    }

    #[test]
    fn a_string_or_template_inside_a_template_hole_does_not_end_the_hole() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];
        let run = |name: &str| script(&vitest, "a.test.js", name);

        for name in [
            r#"literal ${"}"} inside hole"#,
            r"literal ${'}'} inside hole",
            "literal ${`}`} inside hole",
            r#"literal ${"\"}"} inside hole"#,
            r#"literal ${`x ${"}"} y`} inside hole"#,
            r"literal ${f('}', {a: 1}.a)} inside hole",
        ] {
            assert_eq!(
                run(name).as_deref(),
                Some(r#"npx vitest run a.test.js -t "literal .* inside hole""#),
                "{name}"
            );
        }
        assert_eq!(
            run(r#"a ${"} b"#).as_deref(),
            Some(r#"npx vitest run a.test.js -t "a \\\$\\{\"\\} b""#)
        );
    }

    #[test]
    fn a_mocha_name_cuts_at_a_character_that_would_split_the_command_line() {
        let mocha = [("package.json", r#"{"devDependencies":{"mocha":"^10"}}"#)];
        let run = |name: &str| script(&mocha, "a.test.js", name);

        assert_eq!(
            run(r"multi\nline name").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "line name""#)
        );
        assert_eq!(
            run(r"carriage\rreturn").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "carriage""#)
        );
        assert_eq!(
            run(r"backspace \b and formfeed \f").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep " and formfeed ""#)
        );
        assert_eq!(
            run("real\nnewline").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "newline""#)
        );
        assert_eq!(
            run("ab\u{2028}cdef\u{85}gh").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "cdef""#)
        );
        assert_eq!(
            run(r"a\nb\fc").as_deref(),
            Some(r#"npx mocha a.test.js --fgrep "a""#)
        );
        assert_eq!(run(r"\n\f").as_deref(), Some("npx mocha a.test.js"));
        assert_eq!(
            run(r"tab\there").as_deref(),
            Some("npx mocha a.test.js --fgrep \"tab\there\"")
        );
    }

    #[test]
    fn a_vitest_or_jest_name_writes_a_unicode_line_separator_as_an_escape() {
        let vitest = [("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)];

        assert_eq!(
            script(&vitest, "a.test.js", "a\u{2028}b\u{2029}c").as_deref(),
            Some(r#"npx vitest run a.test.js -t "a\\u2028b\\u2029c""#)
        );
    }

    #[test]
    fn a_go_test_below_the_root_changes_into_the_folder_of_its_go_mod() {
        let module = project(&[
            ("svc/go.mod", "module example.com/svc\n"),
            ("my svc/go.mod", "module example.com/mine\n"),
            ("a/b/go.mod", "module example.com/ab\n"),
        ]);

        let run = |path: &str| command(&module, "go", path, "TestAdd", None);

        assert_eq!(
            run("svc/pkg/m_test.go").as_deref(),
            Some("cd svc && go test ./pkg -run '^TestAdd$'")
        );
        assert_eq!(
            run("svc/m_test.go").as_deref(),
            Some("cd svc && go test ./. -run '^TestAdd$'")
        );
        assert_eq!(
            run("my svc/pkg/m_test.go").as_deref(),
            Some("cd 'my svc' && go test ./pkg -run '^TestAdd$'")
        );
        assert_eq!(
            run("a/b/c/m_test.go").as_deref(),
            Some("cd a/b && go test ./c -run '^TestAdd$'")
        );
    }

    #[test]
    fn a_script_test_below_the_root_changes_into_the_folder_of_its_package_json() {
        let run = |manifest: &str, dir: &str| {
            script(
                &[(&format!("{dir}/package.json"), manifest)],
                &format!("{dir}/test/x.test.js"),
                "case",
            )
        };

        assert_eq!(
            run(r#"{"devDependencies":{"vitest":"^1"}}"#, "packages/a").as_deref(),
            Some(r#"cd packages/a && npx vitest run test/x.test.js -t "case""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"jest":"^29"}}"#, "packages/a").as_deref(),
            Some(r#"cd packages/a && npx jest --runTestsByPath test/x.test.js -t "case""#)
        );
        assert_eq!(
            run(r#"{"devDependencies":{"mocha":"^10"}}"#, "web").as_deref(),
            Some(r#"cd web && npx mocha test/x.test.js --fgrep "case""#)
        );
        assert_eq!(
            run(r#"{"scripts":{"test":"node --test"}}"#, "my pkg").as_deref(),
            Some(r#"cd 'my pkg' && node --test-name-pattern "^case\$" test/x.test.js"#)
        );
    }

    #[test]
    fn a_rust_test_changes_into_its_crate_unless_the_root_manifest_is_a_workspace() {
        let member = "[package]\nname = \"inner\"\n";
        let no_root = project(&[("crates/n/Cargo.toml", member)]);
        let plain_root = project(&[
            ("Cargo.toml", "[package]\nname = \"outer\"\n"),
            ("crates/n/Cargo.toml", member),
        ]);
        let workspace_root = project(&[
            ("Cargo.toml", "[workspace]\nmembers = [\"crates/n\"]\n"),
            ("crates/n/Cargo.toml", member),
        ]);
        let unnamed = project(&[("crates/n/Cargo.toml", "[workspace]\nmembers = []\n")]);

        let run =
            |project: &tempfile::TempDir, path: &str| command(project, "rust", path, "case", None);

        assert_eq!(
            run(&no_root, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner case")
        );
        assert_eq!(
            run(&no_root, "crates/n/tests/it.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner --test it case")
        );
        assert_eq!(
            run(&plain_root, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test -p inner case")
        );
        assert_eq!(
            run(&plain_root, "src/lib.rs").as_deref(),
            Some("cargo test -p outer case")
        );
        assert_eq!(
            run(&workspace_root, "crates/n/src/lib.rs").as_deref(),
            Some("cargo test -p inner case")
        );
        assert_eq!(
            run(&unnamed, "crates/n/src/lib.rs").as_deref(),
            Some("cd crates/n && cargo test case")
        );
    }

    #[test]
    fn a_python_test_below_the_root_runs_from_the_root_by_its_path() {
        let nested_project = project(&[("pkg/pyproject.toml", "[project]\nname = \"p\"\n")]);

        assert_eq!(
            command(
                &nested_project,
                "python",
                "pkg/tests/test_a.py",
                "test_one",
                None
            )
            .as_deref(),
            Some("pytest pkg/tests/test_a.py::test_one")
        );
    }

    #[test]
    fn java_tests_run_through_the_nearest_maven_or_gradle_build() {
        let maven = project(&[("app/pom.xml", "<project/>")]);
        let wrapped = project(&[("gradlew", ""), ("app/build.gradle", "")]);
        let kotlin_dsl = project(&[("gradlew", ""), ("app/build.gradle.kts", "")]);
        let bare = project(&[("app/build.gradle", "")]);
        let both = project(&[("pom.xml", "<project/>"), ("build.gradle", "")]);
        let none = project(&[]);
        let path = "app/src/test/java/FooTest.java";

        let run =
            |project: &tempfile::TempDir| command(project, "java", path, "checks", Some("FooTest"));

        assert_eq!(
            run(&maven).as_deref(),
            Some("cd app && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&wrapped).as_deref(),
            Some("cd app && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&kotlin_dsl).as_deref(),
            Some("cd app && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&bare).as_deref(),
            Some("cd app && gradle test --tests 'FooTest.checks'")
        );
        assert_eq!(
            command(&both, "java", path, "checks", Some("FooTest")).as_deref(),
            Some("mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(run(&none), None);
        assert_eq!(command(&maven, "java", path, "checks", None), None);
    }

    #[test]
    fn csharp_tests_run_through_dotnet_test_with_a_class_filter() {
        let empty = project(&[]);

        assert_eq!(
            command(
                &empty,
                "csharp",
                "T/FooTests.cs",
                "Checks",
                Some("FooTests")
            )
            .as_deref(),
            Some("dotnet test --filter \"FullyQualifiedName~FooTests.Checks\"")
        );
        assert_eq!(
            command(&empty, "csharp", "T/FooTests.cs", "Checks", None),
            None
        );
    }

    #[test]
    fn other_languages_have_no_command() {
        let empty = project(&[]);

        for language in ["ruby", "php", "kotlin", "swift", "cpp", "vue"] {
            assert_eq!(
                command(&empty, language, "tests/a_test.x", "case", Some("A")),
                None,
                "{language}"
            );
        }
    }

    #[test]
    fn a_broader_command_uses_only_a_configured_package_script() {
        let configured = project(&[("package.json", r#"{"scripts":{"test":"node --test"}}"#)]);
        let unconfigured = project(&[("package.json", r#"{"scripts":{"build":"node"}}"#)]);
        assert_eq!(
            CommandFinder::new(configured.path()).broader_test_command(None),
            Some("npm test".into())
        );
        assert_eq!(
            CommandFinder::new(unconfigured.path()).broader_test_command(None),
            None
        );
    }

    #[test]
    fn a_broader_command_uses_the_nearest_configured_folder() {
        let project = project(&[
            ("package.json", r#"{"scripts":{"test":"node --test"}}"#),
            (
                "packages/test suite/package.json",
                r#"{"scripts":{"test":"node --test"}}"#,
            ),
        ]);
        let mut finder = CommandFinder::new(project.path());

        assert_eq!(
            finder.broader_test_command(Some("packages/test suite/test/a.test.js")),
            Some("cd 'packages/test suite' && npm test".into())
        );
    }

    #[test]
    fn a_broader_python_command_requires_pytest_configuration() {
        let configured = project(&[(
            "pyproject.toml",
            "[project]\nname = 'configured'\n[tool.pytest.ini_options]\npython_files = ['test_*.py']\n",
        )]);
        let unconfigured = project(&[("pyproject.toml", "[project]\nname = 'plain'\n")]);

        assert_eq!(
            CommandFinder::new(configured.path()).broader_test_command(Some("tests/test_a.py")),
            Some("pytest".into())
        );
        assert_eq!(
            CommandFinder::new(unconfigured.path()).broader_test_command(Some("tests/test_a.py")),
            None
        );
    }

    #[test]
    fn broader_dotnet_command_recognizes_solutions_and_test_project_evidence() {
        for marker in ["app.sln", "app.slnx"] {
            let project = project(&[(marker, "")]);
            assert_eq!(
                CommandFinder::new(project.path()).broader_test_command(None),
                Some("dotnet test".into()),
                "{marker}"
            );
        }

        for marker in ["app.csproj", "app.fsproj", "app.vbproj"] {
            for source in [
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
                "<Project><ItemGroup><PackageReference Include=\"Microsoft.NET.Test.Sdk\" Version=\"17.0\" /></ItemGroup></Project>",
            ] {
                let project = project(&[(marker, source)]);
                assert_eq!(
                    CommandFinder::new(project.path()).broader_test_command(None),
                    Some("dotnet test".into()),
                    "{marker} {source}"
                );
            }
        }
    }

    #[test]
    fn broader_dotnet_command_does_not_treat_a_production_project_as_test_scope() {
        for source in [
            "<Project />",
            "<Project><PropertyGroup><IsTestProject>false</IsTestProject></PropertyGroup></Project>",
        ] {
            let production_only = project(&[("app.csproj", source)]);
            assert_eq!(
                CommandFinder::new(production_only.path()).broader_test_command(None),
                None,
                "{source}"
            );
        }

        let sibling_test = project(&[
            ("app.csproj", "<Project />"),
            (
                "tests/tests.csproj",
                "<Project><PropertyGroup><IsTestProject>true</IsTestProject></PropertyGroup></Project>",
            ),
        ]);
        assert_eq!(
            CommandFinder::new(sibling_test.path()).broader_test_command(Some("src/app.cs")),
            None
        );
    }

    #[test]
    fn gradle_broader_command_uses_the_native_windows_wrapper() {
        for (windows, gradlew, gradlew_bat, expected) in [
            (false, true, true, "./gradlew test"),
            (true, true, true, "gradlew.bat test"),
            (true, false, true, "gradlew.bat test"),
            (false, false, true, "gradle test"),
            (true, false, false, "gradle test"),
        ] {
            assert_eq!(gradle_test_command(windows, gradlew, gradlew_bat), expected);
        }
    }

    #[test]
    fn a_marker_file_is_read_once_per_folder_for_the_life_of_the_finder() {
        let project = project(&[("package.json", r#"{"devDependencies":{"vitest":"^1"}}"#)]);
        let mut finder = CommandFinder::new(project.path());
        let spec = |path| TestSpec {
            language: "typescript",
            kind: "function",
            path,
            line: 1,
            name: "case",
            classes: &[],
        };
        let first = finder.command(&spec("a.test.ts"));

        fs::write(
            project.path().join("package.json"),
            r#"{"devDependencies":{"jest":"^29"}}"#,
        )
        .unwrap();
        let second = finder.command(&spec("b.test.ts"));

        assert_eq!(
            first.as_deref(),
            Some(r#"npx vitest run a.test.ts -t "case""#)
        );
        assert_eq!(
            second.as_deref(),
            Some(r#"npx vitest run b.test.ts -t "case""#)
        );
    }

    fn at(
        project: &tempfile::TempDir,
        language: &str,
        kind: &str,
        path: &str,
        name: &str,
    ) -> Option<String> {
        let text = fs::read_to_string(project.path().join(path)).unwrap();
        let line = text
            .lines()
            .position(|line| line.contains(&format!("{name}(")))
            .unwrap()
            + 1;
        CommandFinder::new(project.path()).command(&TestSpec {
            language,
            kind,
            path,
            line,
            name,
            classes: &[],
        })
    }

    fn pytest(project: &tempfile::TempDir, path: &str, classes: &[(&str, bool)]) -> Option<String> {
        let classes: Vec<TestClass> = classes
            .iter()
            .map(|(name, test_case)| TestClass {
                name,
                test_case: *test_case,
            })
            .collect();
        CommandFinder::new(project.path()).command(&TestSpec {
            language: "python",
            kind: "method",
            path,
            line: 1,
            name: "test_m",
            classes: &classes,
        })
    }

    #[test]
    fn a_go_benchmark_runs_with_the_bench_flag_and_no_test() {
        let module = project(&[("go.mod", "module m\n")]);

        assert_eq!(
            command(&module, "go", "pkg/run_test.go", "BenchmarkRun", None).as_deref(),
            Some("go test ./pkg -bench '^BenchmarkRun$' -run '^$'")
        );
        assert_eq!(
            command(&module, "go", "run_test.go", "Benchmark_snake", None).as_deref(),
            Some("go test ./. -bench '^Benchmark_snake$' -run '^$'")
        );
    }

    #[test]
    fn a_go_fuzz_target_and_a_test_run_by_name_when_the_go_tool_reads_the_name_as_one() {
        let module = project(&[("go.mod", "module m\n")]);

        for name in [
            "TestRun",
            "Test_snake",
            "Test",
            "FuzzRun",
            "Fuzz",
            "TestÜber",
        ] {
            assert_eq!(
                command(&module, "go", "pkg/run_test.go", name, None).as_deref(),
                Some(format!("go test ./pkg -run '^{name}$'").as_str()),
                "{name}"
            );
        }
    }

    #[test]
    fn a_go_function_the_go_tool_does_not_run_by_name_has_no_command() {
        let module = project(&[("go.mod", "module m\n")]);

        for name in [
            "helperNotATest",
            "Testlower",
            "Benchmarkx",
            "TestMain",
            "setupTest",
        ] {
            assert_eq!(
                command(&module, "go", "pkg/run_test.go", name, None),
                None,
                "{name}"
            );
        }
        assert_eq!(command(&module, "go", "pkg/run.go", "TestRun", None), None);
        let method = CommandFinder::new(module.path()).command(&TestSpec {
            language: "go",
            kind: "method",
            path: "pkg/run_test.go",
            line: 1,
            name: "TestSuiteMethod",
            classes: &[],
        });
        assert_eq!(method, None);
    }

    #[test]
    fn a_go_example_runs_only_when_its_body_ends_in_an_output_comment() {
        let module = project(&[
            ("go.mod", "module m\n"),
            (
                "pkg/ex_test.go",
                "package pkg\n\nfunc ExampleOne() {\n\tfmt.Println(1)\n\t// Output: 1\n}\n\nfunc ExampleTwo() {\n\tfmt.Println(2)\n\t// Unordered output: 2\n}\n\nfunc ExampleThree() {\n\tfmt.Println(3)\n}\n\nfunc ExampleFour() {\n\tfmt.Println(4)\n\t// output: 4\n}\n\nfunc ExampleFive() { fmt.Println(5) }\n\nfunc ExampleSix() {\n\t// shows 6\n}\n",
            ),
        ]);
        let run = |name: &str| at(&module, "go", "function", "pkg/ex_test.go", name);

        assert_eq!(
            run("ExampleOne").as_deref(),
            Some("go test ./pkg -run '^ExampleOne$'")
        );
        assert_eq!(
            run("ExampleTwo").as_deref(),
            Some("go test ./pkg -run '^ExampleTwo$'")
        );
        assert_eq!(
            run("ExampleFour").as_deref(),
            Some("go test ./pkg -run '^ExampleFour$'")
        );
        for name in ["ExampleThree", "ExampleFive", "ExampleSix"] {
            assert_eq!(run(name), None, "{name}");
        }
    }

    #[test]
    fn a_maven_or_gradle_test_below_the_root_changes_into_the_folder_of_its_build() {
        let maven = project(&[
            ("mvn/pom.xml", "<project/>"),
            ("my app/pom.xml", "<project/>"),
        ]);
        let local_wrapper = project(&[("gr/gradlew", ""), ("gr/build.gradle", "")]);
        let far_wrapper = project(&[("gradlew", ""), ("a/b/build.gradle.kts", "")]);
        let mid_wrapper = project(&[("a/gradlew", ""), ("a/b/build.gradle", "")]);
        let root_build = project(&[("gradlew", ""), ("build.gradle", "")]);

        let run = |project: &tempfile::TempDir, dir: &str| {
            command(
                project,
                "java",
                &format!("{dir}/src/test/java/FooTest.java"),
                "checks",
                Some("FooTest"),
            )
        };

        assert_eq!(
            run(&maven, "mvn").as_deref(),
            Some("cd mvn && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&maven, "my app").as_deref(),
            Some("cd 'my app' && mvn test -Dtest=FooTest#checks")
        );
        assert_eq!(
            run(&local_wrapper, "gr").as_deref(),
            Some("cd gr && ./gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&far_wrapper, "a/b").as_deref(),
            Some("cd a/b && ../../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            run(&mid_wrapper, "a/b").as_deref(),
            Some("cd a/b && ../gradlew test --tests 'FooTest.checks'")
        );
        assert_eq!(
            command(
                &root_build,
                "java",
                "src/test/java/FooTest.java",
                "checks",
                Some("FooTest")
            )
            .as_deref(),
            Some("./gradlew test --tests 'FooTest.checks'")
        );
    }

    #[test]
    fn a_python_test_in_a_folder_with_square_brackets_runs_from_that_folder() {
        let empty = project(&[]);

        assert_eq!(
            command(
                &empty,
                "python",
                "tests/a[b] (c)/test_p.py",
                "test_m",
                Some("TestK")
            )
            .as_deref(),
            Some("cd 'tests/a[b] (c)' && pytest test_p.py::TestK::test_m")
        );
        assert_eq!(
            command(&empty, "python", "a[b]/test_p.py", "test_free", None).as_deref(),
            Some("cd 'a[b]' && pytest test_p.py::test_free")
        );
        assert_eq!(
            command(&empty, "python", "a::b/test_p.py", "test_free", None).as_deref(),
            Some("cd 'a::b' && pytest test_p.py::test_free")
        );
        assert_eq!(
            command(&empty, "python", "tests/a]b/test_p.py", "test_free", None).as_deref(),
            Some("pytest 'tests/a]b/test_p.py'::test_free")
        );
    }

    #[test]
    fn a_python_test_in_a_file_whose_name_holds_square_brackets_has_no_command() {
        let empty = project(&[]);

        assert_eq!(
            command(&empty, "python", "tests/test_[a].py", "test_m", None),
            None
        );
        assert_eq!(
            command(&empty, "python", "tests/test::a.py", "test_m", None),
            None
        );
    }

    #[test]
    fn a_python_test_in_a_class_pytest_does_not_collect_has_no_command() {
        let empty = project(&[]);
        let path = "tests/test_c.py";

        assert_eq!(pytest(&empty, path, &[("Helper", false)]), None);
        assert_eq!(
            pytest(&empty, path, &[("TestOuter", false), ("Helper", false)]),
            None
        );
        assert_eq!(
            pytest(
                &empty,
                path,
                &[
                    ("TestOuter", false),
                    ("Helper", false),
                    ("TestInner", false)
                ]
            ),
            None
        );
        assert_eq!(
            pytest(&empty, path, &[("Testing", false)]).as_deref(),
            Some("pytest tests/test_c.py::Testing::test_m")
        );
    }

    #[test]
    fn a_python_test_case_subclass_is_collected_under_any_name_but_its_nested_classes_are_not() {
        let empty = project(&[]);
        let path = "tests/test_c.py";

        assert_eq!(
            pytest(&empty, path, &[("FooTests", true)]).as_deref(),
            Some("pytest tests/test_c.py::FooTests::test_m")
        );
        assert_eq!(
            pytest(&empty, path, &[("TestOuter", false), ("Inner", true)]).as_deref(),
            Some("pytest tests/test_c.py::TestOuter::Inner::test_m")
        );
        assert_eq!(
            pytest(&empty, path, &[("FooTests", true), ("TestInner", false)]),
            None
        );
        assert_eq!(
            pytest(&empty, path, &[("TestFoo", true), ("TestInner", false)]),
            None
        );
    }

    #[test]
    fn a_rust_test_marked_ignore_runs_with_the_ignored_flag() {
        let cargo = project(&[
            ("Cargo.toml", "[package]\nname = \"p\"\n"),
            (
                "src/lib.rs",
                "#[cfg(test)]\nmod tests {\n    #[test]\n    #[ignore]\n    fn slow() {}\n\n    #[test]\n    #[ignore = \"needs network\"]\n    fn net() {}\n\n    #[ignore] // later\n    #[test]\n    fn flipped() {}\n\n    #[test]\n    #[ignored_by_us]\n    fn custom() {}\n\n    /// #[ignore]\n    #[test]\n    fn documented() {}\n\n    #[test]\n    fn fast() {}\n\n    #[test] #[ignore]\n    fn one_line() {}\n}\n",
            ),
            (
                "tests/it.rs",
                "#[test]\n#[ignore]\nfn slow_it() {}\n\n#[test]\nfn ok_it() {}\n",
            ),
        ]);
        let run = |path: &str, name: &str| at(&cargo, "rust", "function", path, name);

        for name in ["slow", "net", "flipped", "one_line"] {
            assert_eq!(
                run("src/lib.rs", name).as_deref(),
                Some(format!("cargo test -p p {name} -- --ignored").as_str()),
                "{name}"
            );
        }
        for name in ["custom", "documented", "fast"] {
            assert_eq!(
                run("src/lib.rs", name).as_deref(),
                Some(format!("cargo test -p p {name}").as_str()),
                "{name}"
            );
        }
        assert_eq!(
            run("tests/it.rs", "slow_it").as_deref(),
            Some("cargo test -p p --test it slow_it -- --ignored")
        );
        assert_eq!(
            run("tests/it.rs", "ok_it").as_deref(),
            Some("cargo test -p p --test it ok_it")
        );
    }

    #[test]
    fn an_ignored_rust_test_below_the_root_or_without_a_package_keeps_the_ignored_flag() {
        let member = project(&[
            ("crates/n/Cargo.toml", "[package]\nname = \"inner\"\n"),
            ("crates/n/src/lib.rs", "#[test]\n#[ignore]\nfn slow() {}\n"),
        ]);
        let bare = project(&[("src/lib.rs", "#[test]\n#[ignore]\nfn slow() {}\n")]);

        assert_eq!(
            at(&member, "rust", "function", "crates/n/src/lib.rs", "slow").as_deref(),
            Some("cd crates/n && cargo test -p inner slow -- --ignored")
        );
        assert_eq!(
            at(&bare, "rust", "function", "src/lib.rs", "slow").as_deref(),
            Some("cargo test slow -- --ignored")
        );
    }

    #[test]
    fn a_node_test_runs_only_the_named_test_even_when_the_name_holds_regular_expression_characters()
    {
        let files = [("package.json", r#"{"scripts":{"test":"node --test"}}"#)];

        assert_eq!(
            script(&files, "a.test.js", "sums a.b (1+2) [3] $4").as_deref(),
            Some(r#"node --test-name-pattern "^sums a\\.b \\(1\\+2\\) \\[3\\] \\\$4\$" a.test.js"#)
        );
        assert_eq!(
            script(&files, "a.test.js", "/api/users").as_deref(),
            Some(r#"node --test-name-pattern "^/api/users\$" a.test.js"#)
        );
        assert_eq!(
            script(&files, "a.test.js", "handles ${kind} kind").as_deref(),
            Some(r#"node --test-name-pattern "^handles .* kind\$" a.test.js"#)
        );
    }
}
