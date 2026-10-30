//! What a Gradle settings file (`settings.gradle` or `settings.gradle.kts`) says about the
//! projects of its build: the literal `include` and `includeFlat` entries and the literal
//! `project(":a").projectDir = …` moves, read in source order as Gradle runs them.

/// The projects of one Gradle build, as far as its settings file shows them with certainty.
#[derive(Debug)]
pub(super) struct GradleSettings {
    /// Whether the build can have projects besides the root.
    includes: bool,
    /// Whether the file renames, moves, or loads projects in a way this reader cannot follow.
    projects_unknown: bool,
    projects: Vec<Project>,
}

/// An included project. `dir` is its folder relative to the settings folder, with `/`, and can
/// start with `..`. `included` is false when only a condition or a loop includes it. `placed` is
/// false when a condition, a loop, or an unknown folder can give it a folder other than `dir`;
/// `other_dirs` holds the other folders that the file names for it.
#[derive(Debug)]
struct Project {
    path: String,
    dir: String,
    other_dirs: Vec<String>,
    included: bool,
    placed: bool,
}

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    /// A string literal; `None` when it interpolates, escapes, or spans lines.
    Text(Option<String>),
    Symbol(char),
    LineEnd,
}

impl GradleSettings {
    /// A build whose projects cannot be known, such as one with both settings files.
    pub(super) fn unreadable() -> Self {
        Self {
            includes: true,
            projects_unknown: true,
            projects: Vec::new(),
        }
    }

    pub(super) fn parse(source: &str) -> Self {
        let tokens = lex(source);
        let mut settings = Self {
            includes: false,
            projects_unknown: false,
            projects: Vec::new(),
        };
        let mut index = 0;
        while index < tokens.len() {
            let (token, depth) = &tokens[index];
            let top_level = *depth == 0 && starts_statement(&tokens, index);
            match token {
                Token::Word(word) if word == "include" || word == "includeFlat" => {
                    settings.includes = true;
                    if let Some((paths, next)) = include_arguments(&tokens, index + 1) {
                        for path in paths {
                            settings.include(&path, word == "includeFlat", top_level);
                        }
                        index = next;
                        continue;
                    }
                }
                Token::Word(word) if word == "project" => {
                    if let Some((path, dir, next)) = project_dir_assignment(&tokens, index) {
                        settings.move_project(&path, &dir, top_level);
                        index = next;
                        continue;
                    }
                }
                Token::Word(word)
                    if matches!(
                        word.as_str(),
                        "projectDir" | "setProjectDir" | "buildFileName" | "setBuildFileName"
                    ) =>
                {
                    settings.projects_unknown = true;
                }
                Token::Word(word)
                    if (word == "name" || word == "setName")
                        && renames_a_project(&tokens, index) =>
                {
                    settings.projects_unknown = true;
                }
                Token::Word(word) if word == "apply" && applies_another_file(&tokens, index) => {
                    settings.includes = true;
                    settings.projects_unknown = true;
                }
                _ => {}
            }
            index += 1;
        }
        settings
    }

    /// The test task of the project whose build file is in `build_dir`, for a test in `folder`.
    /// Both folders and `settings_dir` are relative to the project root. `test` for a build with
    /// one project, `:test` for the root of a build with more, `:a:b:test` for an included
    /// project, and `None` when the settings cannot name the project that holds `folder`.
    pub(super) fn test_task(
        &self,
        settings_dir: &str,
        build_dir: &str,
        folder: &str,
    ) -> Option<String> {
        if self.projects_unknown {
            return None;
        }
        let project_between = self.projects.iter().any(|project| {
            project.dirs(settings_dir).any(|dir| {
                dir.as_deref().is_some_and(|dir| {
                    dir != build_dir && holds(build_dir, dir) && holds(dir, folder)
                })
            })
        });
        if project_between {
            return None;
        }
        if build_dir == settings_dir {
            return Some(if self.includes { ":test" } else { "test" }.to_string());
        }
        let mut matching = self.projects.iter().filter(|project| {
            project
                .dirs(settings_dir)
                .any(|dir| dir.as_deref() == Some(build_dir))
        });
        let project = matching.next()?;
        (matching.next().is_none() && project.included && project.placed)
            .then(|| format!("{}:test", project.path))
    }

    /// Whether a project of this file can have `dir`, relative to the project root, as its
    /// folder, when the file is in `settings_dir`.
    pub(super) fn names_dir(&self, settings_dir: &str, dir: &str) -> bool {
        self.projects.iter().any(|project| {
            project
                .dirs(settings_dir)
                .any(|candidate| candidate.as_deref() == Some(dir))
        })
    }

    /// Adds the project `path` and, as Gradle does, each project above it. A new project's
    /// folder is its name inside the folder its parent has at that point, so it is placed for
    /// certain only when its parent is. An include of a project that exists keeps its folder: it
    /// can make the project included for certain, never placed for certain.
    fn include(&mut self, path: &str, flat: bool, certain: bool) {
        let names: Vec<&str> = path.strip_prefix(':').unwrap_or(path).split(':').collect();
        if names.iter().any(|name| name.is_empty()) {
            return;
        }
        if flat {
            if let [name] = names.as_slice() {
                self.include_flat(name, certain);
            }
            return;
        }
        let mut parent_path = String::new();
        let mut parent: Option<(String, Vec<String>, bool)> = None;
        for name in names {
            let path = format!("{parent_path}:{name}");
            let below = |dir: &str| {
                if dir.is_empty() {
                    name.to_string()
                } else {
                    format!("{dir}/{name}")
                }
            };
            let (dir, other_dirs, placed) = match &parent {
                Some((dir, other_dirs, placed)) => (
                    below(dir),
                    other_dirs.iter().map(|dir| below(dir)).collect(),
                    *placed,
                ),
                None => (name.to_string(), Vec::new(), true),
            };
            let project = match self
                .projects
                .iter()
                .position(|project| project.path == path)
            {
                Some(found) => {
                    self.projects[found].included |= certain;
                    &self.projects[found]
                }
                None => {
                    self.projects.push(Project {
                        path: path.clone(),
                        dir,
                        other_dirs,
                        included: certain,
                        placed,
                    });
                    &self.projects[self.projects.len() - 1]
                }
            };
            parent = Some((
                project.dir.clone(),
                project.other_dirs.clone(),
                project.placed,
            ));
            parent_path = path;
        }
    }

    /// `includeFlat name`: the project `:name` in the folder `name` beside the settings folder.
    /// Under a condition the project can also be the `name` folder that a later
    /// `include 'name:child'` gives it.
    fn include_flat(&mut self, name: &str, certain: bool) {
        let path = format!(":{name}");
        let dir = format!("../{name}");
        match self
            .projects
            .iter_mut()
            .find(|project| project.path == path)
        {
            Some(project) => {
                project.included |= certain;
                if project.dir != dir {
                    project.placed = false;
                    project.other_dirs.push(dir);
                }
            }
            None => self.projects.push(Project {
                path,
                dir,
                other_dirs: if certain {
                    Vec::new()
                } else {
                    vec![name.to_string()]
                },
                included: certain,
                placed: certain,
            }),
        }
    }

    /// A literal move of `path` to `dir`. A certain move places the project there; a move under
    /// a condition adds `dir` as another folder it can have. A folder outside the project root
    /// leaves the project unplaced.
    fn move_project(&mut self, path: &str, dir: &str, certain: bool) {
        let path = if path.starts_with(':') {
            path.to_string()
        } else {
            format!(":{path}")
        };
        let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.path == path)
        else {
            self.projects_unknown = true;
            return;
        };
        if dir.starts_with('/') || dir.contains(':') {
            project.placed = false;
        } else if certain {
            project.dir = dir.to_string();
            project.other_dirs.clear();
            project.placed = true;
        } else {
            project.other_dirs.push(dir.to_string());
            project.placed = false;
        }
    }
}

impl Project {
    /// Every folder the project can have, relative to the project root; `None` for one that
    /// leaves the root.
    fn dirs<'a>(&'a self, settings_dir: &'a str) -> impl Iterator<Item = Option<String>> + 'a {
        std::iter::once(&self.dir)
            .chain(&self.other_dirs)
            .map(move |dir| joined(settings_dir, dir))
    }
}

/// `rel` inside `base`, a folder relative to the project root, with `.` and `..` resolved;
/// `None` when it leaves the project root.
fn joined(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in rel.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

/// Whether the folder `outer` is `inner` or a folder above it.
fn holds(outer: &str, inner: &str) -> bool {
    outer.is_empty()
        || inner == outer
        || inner
            .strip_prefix(outer)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether the token at `index` starts a statement: it follows the start of the file, `;`, or a
/// line end after a line that closes its statement, also after a `settings.` receiver.
fn starts_statement(tokens: &[(Token, usize)], index: usize) -> bool {
    let mut start = index;
    if start >= 2
        && tokens[start - 1].0 == Token::Symbol('.')
        && tokens[start - 2].0 == Token::Word("settings".to_string())
    {
        start -= 2;
    }
    if start == 0 {
        return true;
    }
    match tokens[start - 1].0 {
        Token::Symbol(';') => true,
        Token::LineEnd => !leaves_statement_open(tokens, start - 1),
        _ => false,
    }
}

/// Whether the lines before `line_end` leave a statement open, so that the next line belongs to
/// it: they end with `else`, with an operator or a comma, or with the condition of an `if`,
/// `for`, or `while` that has no braces (`if (ci)` and then `include(":a")` on the next line).
fn leaves_statement_open(tokens: &[(Token, usize)], line_end: usize) -> bool {
    let Some(last) = tokens[..line_end]
        .iter()
        .rposition(|(token, _)| *token != Token::LineEnd)
    else {
        return false;
    };
    match &tokens[last].0 {
        Token::Word(word) => word == "else",
        Token::Symbol(')') => closes_a_control_head(tokens, last),
        Token::Symbol(';' | '{' | '}' | ']') | Token::Text(_) | Token::LineEnd => false,
        Token::Symbol(_) => true,
    }
}

/// Whether the `)` at `close` ends the condition of an `if`, `for`, or `while`.
fn closes_a_control_head(tokens: &[(Token, usize)], close: usize) -> bool {
    let mut open_parens = 0usize;
    let mut index = close;
    loop {
        match tokens[index].0 {
            Token::Symbol(')') => open_parens += 1,
            Token::Symbol('(') => {
                open_parens -= 1;
                if open_parens == 0 {
                    break;
                }
            }
            _ => {}
        }
        if index == 0 {
            return false;
        }
        index -= 1;
    }
    tokens[..index]
        .iter()
        .rev()
        .find(|(token, _)| *token != Token::LineEnd)
        .is_some_and(|(token, _)| {
            matches!(token, Token::Word(word) if matches!(word.as_str(), "if" | "for" | "while"))
        })
}

/// The literal paths of an `include` call whose arguments start at `start`: `("a", ":b")` or the
/// Groovy form `'a', ':b'`, with the index after them. `None` when any argument is not a plain
/// string literal.
fn include_arguments(tokens: &[(Token, usize)], start: usize) -> Option<(Vec<String>, usize)> {
    let at = |index: usize| tokens.get(index).map(|(token, _)| token);
    let skip_line_ends = |mut index: usize| {
        while at(index) == Some(&Token::LineEnd) {
            index += 1;
        }
        index
    };
    let mut paths = Vec::new();
    let mut index = start;
    if at(index) == Some(&Token::Symbol('(')) {
        index += 1;
        loop {
            index = skip_line_ends(index);
            match at(index)? {
                Token::Symbol(')') if !paths.is_empty() => return Some((paths, index + 1)),
                Token::Text(Some(path)) => paths.push(path.clone()),
                _ => return None,
            }
            index = skip_line_ends(index + 1);
            match at(index)? {
                Token::Symbol(',') => index += 1,
                Token::Symbol(')') => return Some((paths, index + 1)),
                _ => return None,
            }
        }
    }
    loop {
        let Some(Token::Text(Some(path))) = at(index) else {
            return None;
        };
        paths.push(path.clone());
        index += 1;
        if at(index) != Some(&Token::Symbol(',')) {
            return Some((paths, index));
        }
        index = skip_line_ends(index + 1);
    }
}

/// `project("p").projectDir = <folder>` at `index`, where the folder is `file("d")`,
/// `File(settingsDir, "d")`, `new File(rootDir, "d")`, or `settingsDir.resolve("d")`: the path
/// `p`, the folder `d` relative to the settings folder, and the index after the statement.
fn project_dir_assignment(
    tokens: &[(Token, usize)],
    index: usize,
) -> Option<(String, String, usize)> {
    let at = |offset: usize| tokens.get(index + offset).map(|(token, _)| token);
    let symbol = |offset: usize, symbol: char| at(offset) == Some(&Token::Symbol(symbol));
    let word = |offset: usize, word: &str| matches!(at(offset), Some(Token::Word(w)) if w == word);
    let text = |offset: usize| match at(offset) {
        Some(Token::Text(Some(text))) => Some(text.clone()),
        _ => None,
    };
    let root = |offset: usize| word(offset, "settingsDir") || word(offset, "rootDir");
    if !(symbol(1, '(') && symbol(3, ')') && symbol(4, '.') && word(5, "projectDir")) {
        return None;
    }
    let path = text(2)?;
    if !symbol(6, '=') || symbol(7, '=') {
        return None;
    }
    let value = 7;
    let (dir, end) = if word(value, "file") && symbol(value + 1, '(') && symbol(value + 3, ')') {
        (text(value + 2)?, value + 4)
    } else if root(value)
        && symbol(value + 1, '.')
        && word(value + 2, "resolve")
        && symbol(value + 3, '(')
        && symbol(value + 5, ')')
    {
        (text(value + 4)?, value + 6)
    } else {
        let file = if word(value, "new") { value + 1 } else { value };
        if word(file, "File")
            && symbol(file + 1, '(')
            && root(file + 2)
            && symbol(file + 3, ',')
            && symbol(file + 5, ')')
        {
            (text(file + 4)?, file + 6)
        } else {
            return None;
        }
    };
    Some((path, dir, index + end))
}

/// Whether `name` at `index` renames a project other than the root: `x.name = …` or
/// `x.setName(…)` where `x` is not `rootProject`.
fn renames_a_project(tokens: &[(Token, usize)], index: usize) -> bool {
    if index < 1 || tokens[index - 1].0 != Token::Symbol('.') {
        return false;
    }
    let on_root = index >= 2 && tokens[index - 2].0 == Token::Word("rootProject".to_string());
    let next = |offset: usize| tokens.get(index + offset).map(|(token, _)| token);
    let assigns = match &tokens[index].0 {
        Token::Word(word) if word == "setName" => next(1) == Some(&Token::Symbol('(')),
        _ => next(1) == Some(&Token::Symbol('=')) && next(2) != Some(&Token::Symbol('=')),
    };
    assigns && !on_root
}

/// Whether `apply` at `index` applies another script (`apply from: …`, `apply(from = …)`),
/// which can include more projects.
fn applies_another_file(tokens: &[(Token, usize)], index: usize) -> bool {
    let next = |offset: usize| tokens.get(index + offset).map(|(token, _)| token);
    let from = |token: Option<&Token>| matches!(token, Some(Token::Word(word)) if word == "from");
    from(next(1)) || (next(1) == Some(&Token::Symbol('(')) && from(next(2)))
}

/// The tokens of a settings script with the brace depth of each. Comments are dropped; a
/// comment over several lines counts as a line end.
fn lex(source: &str) -> Vec<(Token, usize)> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut depth = 0usize;
    let mut index = 0;
    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();
        match c {
            '\n' => {
                tokens.push((Token::LineEnd, depth));
                index += 1;
            }
            c if c.is_whitespace() => index += 1,
            '/' if next == Some('/') => {
                while chars.get(index).is_some_and(|&c| c != '\n') {
                    index += 1;
                }
            }
            '/' if next == Some('*') => {
                let start = index;
                index += 2;
                while index < chars.len()
                    && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index = (index + 2).min(chars.len());
                if chars[start..index].contains(&'\n') {
                    tokens.push((Token::LineEnd, depth));
                }
            }
            '"' | '\'' => {
                let (token, end) = string_literal(&chars, index, c);
                tokens.push((token, depth));
                index = end;
            }
            '{' => {
                tokens.push((Token::Symbol(c), depth));
                depth += 1;
                index += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                tokens.push((Token::Symbol(c), depth));
                index += 1;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = index;
                while chars
                    .get(index)
                    .is_some_and(|&c| c.is_alphanumeric() || c == '_')
                {
                    index += 1;
                }
                tokens.push((Token::Word(chars[start..index].iter().collect()), depth));
            }
            c => {
                tokens.push((Token::Symbol(c), depth));
                index += 1;
            }
        }
    }
    tokens
}

/// The string literal that opens with `quote` at `start`, and the index after it. A triple-quoted
/// literal, an escape, a `$` in a double-quoted literal, or a literal cut by a line end gives
/// `Text(None)`.
fn string_literal(chars: &[char], start: usize, quote: char) -> (Token, usize) {
    let triple = chars.get(start + 1) == Some(&quote) && chars.get(start + 2) == Some(&quote);
    if triple {
        let mut index = start + 3;
        while index < chars.len() {
            if chars[index..].starts_with(&[quote, quote, quote]) {
                return (Token::Text(None), index + 3);
            }
            index += 1;
        }
        return (Token::Text(None), chars.len());
    }
    let mut index = start + 1;
    let mut plain = true;
    while let Some(&c) = chars.get(index) {
        match c {
            c if c == quote => {
                let text: String = chars[start + 1..index].iter().collect();
                return (Token::Text(plain.then_some(text)), index + 1);
            }
            '\\' => {
                plain = false;
                index += 2;
            }
            '\n' => return (Token::Text(None), index),
            '$' if quote == '"' => {
                plain = false;
                index += 1;
            }
            _ => index += 1,
        }
    }
    (Token::Text(None), chars.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(settings: &str, build_dir: &str, folder: &str) -> Option<String> {
        GradleSettings::parse(settings).test_task("", build_dir, folder)
    }

    #[test]
    fn kotlin_dsl_includes_name_nested_projects_and_their_parents() {
        let settings = "rootProject.name = \"moshi-root\"\n\ninclude(\":moshi-kotlin-tests\")\n\ninclude(\":moshi-kotlin-tests:codegen-only\")\n";

        assert_eq!(
            task(
                settings,
                "moshi-kotlin-tests",
                "moshi-kotlin-tests/src/test/kotlin"
            )
            .as_deref(),
            Some(":moshi-kotlin-tests:test")
        );
        assert_eq!(
            task(
                settings,
                "moshi-kotlin-tests/codegen-only",
                "moshi-kotlin-tests/codegen-only/src/test/kotlin"
            )
            .as_deref(),
            Some(":moshi-kotlin-tests:codegen-only:test")
        );
        assert_eq!(
            task(settings, "", "src/test/kotlin").as_deref(),
            Some(":test")
        );
    }

    #[test]
    fn groovy_dsl_includes_take_several_arguments_with_or_without_parentheses() {
        let settings =
            "include 'core', ':lib:json',\n    'app'\ninclude(\n  ':cli',\n  'tools:gen',\n)\n";

        for (dir, expected) in [
            ("core", ":core:test"),
            ("lib", ":lib:test"),
            ("lib/json", ":lib:json:test"),
            ("app", ":app:test"),
            ("cli", ":cli:test"),
            ("tools/gen", ":tools:gen:test"),
        ] {
            assert_eq!(
                task(settings, dir, &format!("{dir}/src/test/java")).as_deref(),
                Some(expected),
                "{dir}"
            );
        }
    }

    #[test]
    fn a_project_dir_override_moves_the_project_and_the_children_included_after_it() {
        let groovy = "include ':api'\nproject(':api').projectDir = new File(settingsDir, 'modules/api')\ninclude ':api:v2'\n";
        let kotlin = "include(\":api\")\nproject(\":api\").projectDir = file(\"modules/api\")\n";
        let resolved =
            "include(\":api\")\nproject(\":api\").projectDir = rootDir.resolve(\"modules/api\")\n";

        for settings in [groovy, kotlin, resolved] {
            assert_eq!(
                task(settings, "modules/api", "modules/api/src/test/java").as_deref(),
                Some(":api:test"),
                "{settings}"
            );
            assert_eq!(
                task(settings, "api", "api/src/test/java"),
                None,
                "{settings}"
            );
        }
        assert_eq!(
            task(groovy, "modules/api/v2", "modules/api/v2/src/test/java").as_deref(),
            Some(":api:v2:test")
        );
    }

    #[test]
    fn a_settings_file_with_no_include_names_a_single_project_build() {
        let settings =
            "rootProject.name = 'solo'\n// include ':ghost'\n/* include(\":ghost\") */\n";

        assert_eq!(task(settings, "", "src/test/java").as_deref(), Some("test"));
        assert_eq!(task(settings, "ghost", "ghost/src/test/java"), None);
    }

    #[test]
    fn a_project_the_settings_cannot_name_with_certainty_has_no_task() {
        for settings in [
            "file(\"libs\").listFiles().forEach { include(\":libs:${it.name}\") }\n",
            "if (JavaVersion.current().isJava11Compatible) {\n  include(\":records\")\n}\n",
            "if (ci) include(\":records\")\n",
            "include(\":records\")\nrootProject.children.forEach { it.projectDir = file(\"x/${it.name}\") }\n",
            "include(\":records\")\nproject(\":records\").name = \"renamed\"\n",
            "include(\":records\")\napply(from = \"more-settings.gradle.kts\")\n",
            "include ':records'\nrootProject.children.each { it.buildFileName = \"${it.name}.gradle\" }\n",
            "if (System.getenv(\"CI\") != null)\n    include(\":records\")\n",
            "if (System.getenv('CI'))\n    include ':records'\n",
            "if (System.getenv('CI') &&\n    !System.getenv('LOCAL'))\n\n    include ':records'\n",
            "if (ci) include(\":other\")\nelse\n    include(\":records\")\n",
            "for (name in listOf(\"records\"))\n    include(\":records\")\n",
            "while (more())\n    settings.include(\":records\")\n",
            "include(\":records\")\nif (ci)\n    project(\":records\").projectDir = file(\"elsewhere\")\n",
            "include ':records'\nif (System.getenv('CI'))\n    project(':records').projectDir = new File(settingsDir, 'elsewhere')\n",
        ] {
            assert_eq!(
                task(settings, "records", "records/src/test/java"),
                None,
                "{settings}"
            );
        }
        assert_eq!(
            GradleSettings::unreadable().test_task("", "", "src/test/java"),
            None
        );
    }

    #[test]
    fn an_include_after_a_line_that_closes_its_statement_stays_certain() {
        for settings in [
            "if (ci) println(\"ci\")\ninclude(\":records\")\n",
            "if (ci) {\n    println(\"ci\")\n}\ninclude(\":records\")\n",
            "rootProject.name = \"demo\"\n\ninclude(\":records\")\n",
            "plugins {\n    id(\"x\") version \"1\"\n}\n// if (ci)\ninclude(\":records\")\n",
            "includeBuild(\"tools\")\ninclude ':records'\n",
            "include(\":other\");\ninclude(\":records\")\n",
        ] {
            assert_eq!(
                task(settings, "records", "records/src/test/java").as_deref(),
                Some(":records:test"),
                "{settings}"
            );
        }
    }

    #[test]
    fn a_later_include_never_places_a_project_that_a_condition_moves() {
        let settings = "include ':a'\nif (System.getenv('CI')) {\n  project(':a').projectDir = file('elsewhere')\n}\ninclude ':a:b'\n";

        for (dir, folder) in [
            ("a", "a/src/test/java"),
            ("elsewhere", "elsewhere/src/test/java"),
            ("a/b", "a/b/src/test/java"),
            ("elsewhere/b", "elsewhere/b/src/test/java"),
        ] {
            assert_eq!(task(settings, dir, folder), None, "{dir}");
        }
    }

    #[test]
    fn a_later_include_never_places_a_project_that_a_condition_includes_flat() {
        let settings = GradleSettings::parse(
            "if (System.getenv('CI')) {\n  includeFlat 'x'\n}\ninclude 'x:y'\n",
        );

        for (dir, folder) in [
            ("x", "x/src/test/java"),
            ("x/y", "x/y/src/test/java"),
            ("build/x", "build/x/src/test/java"),
            ("build/x/y", "build/x/y/src/test/java"),
        ] {
            assert_eq!(settings.test_task("build", dir, folder), None, "{dir}");
        }
    }

    #[test]
    fn a_later_include_makes_a_conditional_include_certain_in_its_own_folder() {
        let settings = "if (ci) {\n  include ':a'\n}\ninclude ':a:b'\n";

        assert_eq!(
            task(settings, "a", "a/src/test/java").as_deref(),
            Some(":a:test")
        );
        assert_eq!(
            task(settings, "a/b", "a/b/src/test/java").as_deref(),
            Some(":a:b:test")
        );
    }

    #[test]
    fn a_test_inside_an_included_project_without_its_own_build_file_has_no_task() {
        let settings = "include ':sub'\n";

        assert_eq!(task(settings, "", "sub/src/test/java"), None);
        assert_eq!(
            task(settings, "", "src/test/java").as_deref(),
            Some(":test")
        );
    }

    #[test]
    fn a_settings_file_below_the_root_names_projects_from_its_own_folder() {
        let settings = GradleSettings::parse("include(\":app\")\nincludeFlat(\"shared\")\n");

        assert_eq!(
            settings
                .test_task("build", "build/app", "build/app/src/test/java")
                .as_deref(),
            Some(":app:test")
        );
        assert_eq!(
            settings
                .test_task("build", "shared", "shared/src/test/java")
                .as_deref(),
            Some(":shared:test")
        );
    }
}
