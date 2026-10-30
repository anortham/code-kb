//! What the `.sbt` files of an sbt build folder say about its projects: the literal
//! `lazy val id = project`, `project.in(file("dir"))`, `(project in file("dir"))`, and
//! `Project("id", file("dir"))` definitions.

use super::gradle::{holds, joined};
use super::{CommandFinder, parent_dir, shell_word, single_quoted};
use std::fs;
use std::path::Path;

/// The projects of one sbt build folder, as far as its `.sbt` files show them with certainty.
#[derive(Debug)]
pub(super) struct SbtBuild {
    /// Each project id with its folder relative to the build folder, `""` for the root project.
    projects: Vec<(String, String)>,
    /// Whether the build defines projects in a way this reader cannot follow, such as
    /// `crossProject`, `projectMatrix`, `.withId`, or a folder that is no literal.
    unknown: bool,
}

/// How sbt runs the tests of one project: the build folder it runs from, and the id of the
/// project when it is not the root project.
pub(super) struct SbtRun {
    pub(super) dir: String,
    pub(super) project: Option<String>,
}

impl SbtRun {
    /// The command that runs every test of the project.
    pub(super) fn command(&self) -> String {
        match &self.project {
            Some(project) => format!("sbt {}", shell_word(&format!("{project}/test"))),
            None => "sbt test".to_string(),
        }
    }

    /// The command that runs `task`, a `testOnly` task, in the project.
    pub(super) fn task_command(&self, task: &str) -> String {
        let task = match &self.project {
            Some(project) => format!("{project}/{task}"),
            None => task.to_string(),
        };
        format!("sbt {}", single_quoted(&task))
    }
}

/// A project id and its folder as the definition writes it.
type IdAndDir = (String, String);

#[derive(Debug, PartialEq)]
enum Token {
    Word(String),
    /// A string literal; `None` when it escapes, interpolates, or spans lines.
    Text(Option<String>),
    Symbol(char),
}

impl SbtBuild {
    /// The projects that `sources`, the `.sbt` files of one folder, define. `scala_definitions`
    /// says that a Scala file of the build's `project` folder defines projects.
    pub(super) fn parse(sources: &[String], scala_definitions: bool) -> Self {
        let mut build = Self {
            projects: Vec::new(),
            unknown: scala_definitions,
        };
        for source in sources {
            build.read(&lex(source));
        }
        build
    }

    fn read(&mut self, tokens: &[Token]) {
        let word = |index: usize| match tokens.get(index) {
            Some(Token::Word(word)) => Some(word.as_str()),
            _ => None,
        };
        let symbol = |index: usize, c: char| tokens.get(index) == Some(&Token::Symbol(c));
        let mut index = 0;
        while index < tokens.len() {
            if word(index) == Some("val")
                && let Some(id) = word(index + 1)
                && let Some((project, next)) = definition(tokens, index + 2, id)
            {
                match project {
                    Some((id, dir)) => match joined("", &dir) {
                        Some(dir) => self.projects.push((id, dir)),
                        None => self.unknown = true,
                    },
                    None => self.unknown = true,
                }
                index = next;
                continue;
            }
            let unknown = match word(index) {
                Some(
                    "project" | "Project" | "crossProject" | "CrossProject" | "projectMatrix"
                    | "withId" | "RootProject" | "ProjectRef",
                ) => true,
                Some("in") => {
                    word(index + 1) == Some("file")
                        || (symbol(index + 1, '(') && word(index + 2) == Some("file"))
                }
                _ => false,
            };
            self.unknown |= unknown;
            index += 1;
        }
    }

    /// The id of the deepest project other than the root project whose `src` folder holds
    /// `folder`, for a build in `build_dir`. Both folders are relative to the project root.
    fn project_of(&self, build_dir: &str, folder: &str) -> Option<&str> {
        self.projects
            .iter()
            .filter(|(_, dir)| !dir.is_empty())
            .filter_map(|(id, dir)| Some((id, joined(build_dir, &format!("{dir}/src"))?)))
            .filter(|(_, sources)| holds(sources, folder))
            .max_by_key(|(_, sources)| sources.len())
            .map(|(id, _)| id.as_str())
    }

    /// Whether the build in `build_dir` defines a project other than the root project in the
    /// folder `folder`. Both folders are relative to the project root.
    fn defines_project_in(&self, build_dir: &str, folder: &str) -> bool {
        self.projects
            .iter()
            .any(|(_, dir)| !dir.is_empty() && joined(build_dir, dir).as_deref() == Some(folder))
    }

    fn has_subprojects(&self) -> bool {
        self.projects.iter().any(|(_, dir)| !dir.is_empty())
    }
}

/// The project that the definition after `val id` at `start` makes, and the index after it.
/// `Some((None, _))` for a project definition this reader cannot follow, and `None` when the
/// value is no project.
fn definition(tokens: &[Token], start: usize, id: &str) -> Option<(Option<IdAndDir>, usize)> {
    let word = |index: usize| match tokens.get(index) {
        Some(Token::Word(word)) => Some(word.as_str()),
        _ => None,
    };
    let symbol = |index: usize, c: char| tokens.get(index) == Some(&Token::Symbol(c));
    let text = |index: usize| match tokens.get(index) {
        Some(Token::Text(Some(text))) => Some(text.clone()),
        _ => None,
    };
    let file_at = |index: usize| {
        (word(index) == Some("file") && symbol(index + 1, '(') && symbol(index + 3, ')'))
            .then(|| text(index + 2))
            .flatten()
    };
    let mut index = start;
    if symbol(index, ':') {
        index += 2;
    }
    if !symbol(index, '=') {
        return None;
    }
    index += 1;
    while symbol(index, '(') {
        index += 1;
    }
    match word(index) {
        Some("project") => {
            index += 1;
            if symbol(index, '.') && word(index + 1) == Some("in") {
                if symbol(index + 2, '(')
                    && let Some(dir) = file_at(index + 3)
                    && symbol(index + 7, ')')
                {
                    return Some((Some((id.to_string(), dir)), index + 8));
                }
                return Some((None, index));
            }
            if word(index) == Some("in") {
                if let Some(dir) = file_at(index + 1) {
                    return Some((Some((id.to_string(), dir)), index + 5));
                }
                return Some((None, index));
            }
            Some((Some((id.to_string(), id.to_string())), index))
        }
        Some("Project") => {
            if symbol(index + 1, '(')
                && let Some(project_id) = text(index + 2)
                && symbol(index + 3, ',')
                && let Some(dir) = file_at(index + 4)
                && symbol(index + 8, ')')
            {
                return Some((Some((project_id, dir)), index + 9));
            }
            Some((None, index + 1))
        }
        Some("crossProject" | "CrossProject" | "projectMatrix") => Some((None, index + 1)),
        _ => None,
    }
}

/// The tokens of an `.sbt` file. Comments are dropped, and a backticked name is one word.
fn lex(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(&c) = chars.get(index) {
        let next = chars.get(index + 1).copied();
        match c {
            c if c.is_whitespace() => index += 1,
            '/' if next == Some('/') => {
                while chars.get(index).is_some_and(|&c| c != '\n') {
                    index += 1;
                }
            }
            '/' if next == Some('*') => {
                index += 2;
                while index < chars.len()
                    && !(chars[index] == '*' && chars.get(index + 1) == Some(&'/'))
                {
                    index += 1;
                }
                index = (index + 2).min(chars.len());
            }
            '`' => {
                let start = index + 1;
                index = start;
                while chars.get(index).is_some_and(|&c| c != '`' && c != '\n') {
                    index += 1;
                }
                tokens.push(Token::Word(chars[start..index].iter().collect()));
                index += 1;
            }
            '"' => {
                let (token, end) = string_literal(&chars, index);
                tokens.push(token);
                index = end;
            }
            c if c.is_alphanumeric() || c == '_' => {
                let start = index;
                while chars
                    .get(index)
                    .is_some_and(|&c| c.is_alphanumeric() || c == '_')
                {
                    index += 1;
                }
                tokens.push(Token::Word(chars[start..index].iter().collect()));
            }
            c => {
                tokens.push(Token::Symbol(c));
                index += 1;
            }
        }
    }
    tokens
}

/// The string literal that opens at `start`, and the index after it. A triple-quoted literal, an
/// escape, or a literal cut by a line end gives `Text(None)`. An interpolator such as `s` before
/// the quote is its own word, so the path of `file(s"…")` is no literal.
fn string_literal(chars: &[char], start: usize) -> (Token, usize) {
    if chars.get(start + 1) == Some(&'"') && chars.get(start + 2) == Some(&'"') {
        let mut index = start + 3;
        while index < chars.len() {
            if chars[index..].starts_with(&['"', '"', '"']) {
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
            '"' => {
                let text: String = chars[start + 1..index].iter().collect();
                return (Token::Text(plain.then_some(text)), index + 1);
            }
            '\\' => {
                plain = false;
                index += 2;
            }
            '\n' => return (Token::Text(None), index),
            _ => index += 1,
        }
    }
    (Token::Text(None), chars.len())
}

/// Whether a Scala file in the `project` folder of the build folder `dir` defines projects.
pub(super) fn scala_build_defines_projects(dir: &Path) -> bool {
    fs::read_dir(dir.join("project"))
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".scala"))
        .filter_map(|entry| fs::read_to_string(entry.path()).ok())
        .any(|source| scala_defines_projects(&source))
}

/// Whether a Scala file of a build's `project` folder defines projects, which this reader does
/// not follow.
fn scala_defines_projects(source: &str) -> bool {
    lex(source).iter().any(|token| {
        matches!(token, Token::Word(word) if matches!(
            word.as_str(),
            "project" | "Project" | "crossProject" | "CrossProject" | "projectMatrix"
        ))
    })
}

impl CommandFinder {
    /// How sbt runs the tests of the project that holds `folder`, whose nearest `build.sbt` is in
    /// `build_dir`. A folder under the `src` folder of a project other than the root project
    /// that the `.sbt` files of `build_dir` or of a folder above it define runs from that build
    /// folder as `sbt 'id/testOnly …'`, because sbt started in a project folder loads a separate
    /// build without the settings and the projects of the real one. The walk up stops at a
    /// build folder with a `project` folder, the root of its own build. Any other folder runs in
    /// the root project of `build_dir`. `None` when a build folder on the way defines projects
    /// that this reader cannot follow, when a build above defines a project in `build_dir` and
    /// `folder` is outside its `src`, or when `build_dir` has other projects and `folder` is
    /// outside its `src`.
    pub(super) fn sbt_run(&mut self, build_dir: &str, folder: &str) -> Option<SbtRun> {
        let mut dir = build_dir;
        loop {
            if let Some(build) = self.folder(dir).sbt.as_ref() {
                if build.unknown {
                    return None;
                }
                if let Some(id) = build.project_of(dir, folder) {
                    return Some(SbtRun {
                        dir: dir.to_string(),
                        project: Some(id.to_string()),
                    });
                }
                if build.defines_project_in(dir, build_dir) {
                    return None;
                }
            }
            if dir.is_empty() || self.folder(dir).sbt_build_root {
                break;
            }
            dir = parent_dir(dir);
        }
        let in_root_sources = holds(&joined(build_dir, "src")?, folder);
        let build = self.folder(build_dir).sbt.as_ref()?;
        if build.has_subprojects() && !in_root_sources {
            return None;
        }
        Some(SbtRun {
            dir: build_dir.to_string(),
            project: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(source: &str) -> SbtBuild {
        SbtBuild::parse(&[source.to_string()], false)
    }

    #[test]
    fn literal_project_definitions_name_their_id_and_folder() {
        let parsed = build(
            "ThisBuild / scalaVersion := \"3.3.1\"\n\nlazy val root = (project in file(\".\")).aggregate(core, `web-app`, util, api)\n\nlazy val core = project\n  .settings(name := \"core\")\n\nlazy val `web-app` = project.in(file(\"apps/web\")).dependsOn(core)\n\nlazy val util: Project = (project in file(\"libs/util\"))\n\nval api = Project(\"api-id\", file(\"api\"))\n",
        );

        assert!(!parsed.unknown, "{parsed:?}");
        assert_eq!(
            parsed.projects,
            vec![
                ("root".to_string(), String::new()),
                ("core".to_string(), "core".to_string()),
                ("web-app".to_string(), "apps/web".to_string()),
                ("util".to_string(), "libs/util".to_string()),
                ("api-id".to_string(), "api".to_string()),
            ]
        );
        assert_eq!(parsed.project_of("", "core/src/test/scala"), Some("core"));
        assert_eq!(parsed.project_of("", "apps/web/src/test"), Some("web-app"));
        assert_eq!(parsed.project_of("", "src/test/scala"), None);
        assert_eq!(parsed.project_of("", "apps/src"), None);
    }

    #[test]
    fn project_definitions_the_reader_cannot_follow_make_the_build_unknown() {
        for source in [
            "lazy val core = crossProject(JVMPlatform, JSPlatform).in(file(\"core\"))",
            "lazy val core = projectMatrix.in(file(\"core\"))",
            "lazy val core = project.in(file(coreDir))",
            "lazy val core = (project in file(s\"modules/$name\"))",
            "lazy val core = project.withId(\"other\")",
            "def module(name: String) = Project(name, file(name))",
            "lazy val core = project.settings(x := 1).in(file(\"core\"))",
        ] {
            assert!(build(source).unknown, "{source}");
        }
        assert!(SbtBuild::parse(&[String::new()], true).unknown);
    }

    #[test]
    fn a_single_project_build_has_no_subprojects() {
        let parsed = build(
            "name := \"project\"\n// lazy val core = crossProject\nfork in Test := true\nlibraryDependencies += \"org.scalameta\" %% \"munit\" % \"1.0.0\" % Test\n",
        );

        assert!(!parsed.unknown, "{parsed:?}");
        assert!(!parsed.has_subprojects());
    }

    #[test]
    fn a_scala_build_definition_that_names_projects_is_detected() {
        assert!(scala_defines_projects(
            "object Build { lazy val core = Project(\"core\", file(\"core\")) }"
        ));
        assert!(!scala_defines_projects(
            "object Dependencies { val munit = \"org.scalameta\" %% \"munit\" % \"1.0.0\" }"
        ));
    }
}
