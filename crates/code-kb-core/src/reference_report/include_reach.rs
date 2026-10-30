//! The files a C or C++ site can see: its own file, the headers it includes, and, for a site in
//! a header, the files that include that header and what they include.
//!
//! A definition in a header outside those files never reaches the site, so neither a qualified
//! path nor julie's own link can prove it, and it is no other definition of the site. A function
//! or variable in a source file can still reach the site by linking, and a declaration or an
//! elaborated type name (`struct S;`, `class S *p`) inside them can name an outside definition,
//! so the rule leaves those to the other rules. A source file that another file includes (a
//! template `.cpp` that its header includes, a unity build) sees what its includers see. When an
//! include does not resolve, a header has no indexed includer, or a file holds an include that
//! julie did not record (`#include CONFIG_H`, `#import`, `#include_next`), the files are unknown
//! and the rule decides nothing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, params};

use super::loaded_files::Reach;
use super::{Accounting, Definition, NameRow, QueryError, SiteLabel, Spec, Verdict};

const MAX_INCLUDE_HOPS: usize = 16;
const MAX_INCLUDE_FILES: usize = 2048;
const MAX_INCLUDERS: usize = 512;
const MAX_NAMED_ROWS: usize = 200;

const INCLUDER_FILES_SQL: &str = "SELECT DISTINCT replace(path, '\\', '/') FROM symbols
     WHERE kind = 'import' AND language IN ('c', 'cpp') AND json_valid(metadata_json)
       AND json_extract(metadata_json, '$.form') = '#include'
       AND substr(replace(name, '\\', '/'),
                  length(rtrim(replace(name, '\\', '/'), replace(replace(name, '\\', '/'), '/', ''))) + 1)
           IN (SELECT value FROM json_each(?1))
     LIMIT 513";

const STEP_SYMBOL_SQL: &str = "SELECT replace(path, '\\', '/'), language, kind, name,
            parent_symbol_id IS NULL
     FROM symbols WHERE symbol_id = ?1";

const CALL_RECEIVERS_SQL: &str = "SELECT COALESCE(target_receiver, '')
     FROM pending_relationships WHERE from_symbol_id = ?1 AND target_terminal_name = ?2";

const NAMESPACE_MEMBERS_SQL: &str = "SELECT replace(c.path, '\\', '/'), n.name
     FROM symbols c JOIN symbols n ON c.parent_symbol_id = n.symbol_id
     WHERE c.name = ?1 AND n.kind = 'namespace' AND c.language IN ('c', 'cpp')
       AND c.kind NOT IN ('import', 'export', 'namespace')
     LIMIT 201";

/// The time the include checks of one blast-radius walk may take; past it every step stays.
const WALK_TIME_LIMIT: Duration = Duration::from_secs(3);

const C_NAMED_SQL: &str = "SELECT replace(path, '\\', '/'), kind, body_start_line IS NULL
     FROM symbols
     WHERE name = ?1 AND language IN ('c', 'cpp') AND kind NOT IN ('import', 'export', 'namespace')
     ORDER BY path, start_line
     LIMIT 201";

/// The files a site can see, and whether every include on the way is known.
pub(super) struct IncludeReach {
    files: HashSet<String>,
    complete: bool,
}

#[derive(Default)]
pub(super) struct IncludeCaches {
    reach: HashMap<(String, bool), Rc<IncludeReach>>,
    includes_recorded: HashMap<String, bool>,
    elaborated: HashMap<(String, String), bool>,
    named: HashMap<String, Rc<Vec<NamedRow>>>,
}

struct NamedRow {
    path: String,
    kind: String,
    declaration: bool,
}

fn is_c_family(language: &str) -> bool {
    matches!(language, "c" | "cpp")
}

/// Whether a path names a C or C++ source file, which a build compiles as its own translation
/// unit unless another file includes it.
fn is_c_source(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some(dot) = name.rfind('.').filter(|dot| *dot > 0) else {
        return false;
    };
    let extension = &name[dot + 1..];
    extension == "C"
        || matches!(
            extension.to_ascii_lowercase().as_str(),
            "c" | "cc" | "cpp" | "cxx" | "c++" | "cp" | "m" | "mm" | "cu" | "ino"
        )
}

/// The kinds that never link across translation units: a file must see the definition.
fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class" | "struct" | "union" | "enum" | "interface" | "type"
    )
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn last_segment(name: &str) -> &str {
    name.rsplit("::").next().unwrap_or(name)
}

/// The header that each `#include`, `#include_next`, or `#import` directive of C or C++ source
/// text names, with whether it is written in angle brackets; `None` for a directive that names
/// it through a macro.
fn include_directives(text: &str) -> Vec<Option<(&str, bool)>> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix('#')?.trim_start();
            let rest = ["include_next", "include", "import"]
                .iter()
                .find_map(|directive| rest.strip_prefix(directive))?;
            if !rest.starts_with([' ', '\t', '"', '<']) {
                return None;
            }
            let rest = rest.trim_start();
            let close = match rest.chars().next() {
                Some('<') => '>',
                Some('"') => '"',
                _ => return Some(None),
            };
            Some(
                rest[1..]
                    .find(close)
                    .map(|end| (&rest[1..1 + end], close == '>')),
            )
        })
        .collect()
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The word that ends at `end` after any spaces, and the index where it starts.
fn word_before(bytes: &[u8], end: usize) -> (&[u8], usize) {
    let mut stop = end;
    while stop > 0 && bytes[stop - 1].is_ascii_whitespace() {
        stop -= 1;
    }
    let mut start = stop;
    while start > 0 && is_word_byte(bytes[start - 1]) {
        start -= 1;
    }
    (&bytes[start..stop], start)
}

fn is_tag_keyword(word: &[u8]) -> bool {
    matches!(word, b"class" | b"struct" | b"union" | b"enum")
}

fn is_macro_word(word: &[u8]) -> bool {
    !word.is_empty()
        && word
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || *byte == b'_')
}

/// Whether the text names the type `name` after `class`, `struct`, `union`, or `enum` without
/// defining it there (`struct S;`, `class S *p`, `friend class S;`), which declares a type that a
/// definition in another file can complete.
fn names_type_by_tag(text: &str, name: &str) -> bool {
    let bytes = text.as_bytes();
    let mut from = 0;
    while let Some(found) = text[from..].find(name) {
        let start = from + found;
        let end = start + name.len();
        from = end;
        if start > 0 && is_word_byte(bytes[start - 1])
            || end < bytes.len() && is_word_byte(bytes[end])
        {
            continue;
        }
        let (mut word, word_start) = word_before(bytes, start);
        if is_macro_word(word) && !is_tag_keyword(word) {
            word = word_before(bytes, word_start).0;
        }
        if !is_tag_keyword(word) {
            continue;
        }
        let after = text[end..].trim_start();
        let defines = after.starts_with('{')
            || after.starts_with(':') && !after.starts_with("::")
            || after.starts_with("final");
        if !defines {
            return true;
        }
    }
    false
}

/// The C and C++ steps of a blast-radius walk: a step from a caller to a definition in a file
/// that the caller's file cannot see is no call of it.
pub(crate) struct IncludeSteps<'a> {
    accounting: Accounting<'a>,
    symbols: HashMap<String, Option<StepSymbol>>,
}

#[derive(Clone)]
struct StepSymbol {
    path: String,
    language: String,
    kind: String,
    name: String,
    top_level: bool,
}

impl<'a> IncludeSteps<'a> {
    pub(crate) fn new(conn: &'a Connection, root: &'a Path) -> Self {
        Self {
            accounting: Accounting::new(conn, root, WALK_TIME_LIMIT),
            symbols: HashMap::new(),
        }
    }

    /// Whether the step from `caller` to `origin` stays in the walk.
    pub(crate) fn keeps(&mut self, caller: &str, origin: &str) -> bool {
        if caller == origin {
            return true;
        }
        self.leaves_reach(caller, origin)
            .map(|outside| !outside)
            .unwrap_or(true)
    }

    fn leaves_reach(&mut self, caller: &str, origin: &str) -> Result<bool, QueryError> {
        let (Some(site), Some(target)) = (self.symbol(caller)?, self.symbol(origin)?) else {
            return Ok(false);
        };
        if !is_c_family(&site.language) || !is_c_family(&target.language) {
            return Ok(false);
        }
        if self
            .accounting
            .outside_includes(&site.path, &target.path, &target.kind, &target.name)?
        {
            return Ok(true);
        }
        if !target.top_level {
            return Ok(false);
        }
        self.calls_only_a_namespace_member(caller, &site.path, &target.name)
    }

    /// Whether every call of `name` that `caller` makes is qualified by a namespace that holds a
    /// definition of `name` in a file the caller's file sees (`util::helper()` with `util.h`
    /// included), so none of them calls a top-level definition of that name.
    fn calls_only_a_namespace_member(
        &mut self,
        caller: &str,
        site: &str,
        name: &str,
    ) -> Result<bool, QueryError> {
        let receivers = self.accounting.with_statement(CALL_RECEIVERS_SQL, |stmt| {
            stmt.query_map(params![caller, name], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        if receivers.is_empty() || receivers.iter().any(String::is_empty) {
            return Ok(false);
        }
        let members = self
            .accounting
            .with_statement(NAMESPACE_MEMBERS_SQL, |stmt| {
                stmt.query_map(params![name], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
            })?;
        if members.is_empty() || members.len() > MAX_NAMED_ROWS {
            return Ok(false);
        }
        let reach = self.accounting.site_reach(site)?;
        Ok(receivers.iter().all(|receiver| {
            let receiver = last_segment(receiver);
            members.iter().any(|(path, namespace)| {
                last_segment(namespace) == receiver && (path == site || reach.files.contains(path))
            })
        }))
    }

    fn symbol(&mut self, id: &str) -> Result<Option<StepSymbol>, QueryError> {
        if let Some(known) = self.symbols.get(id) {
            return Ok(known.clone());
        }
        let row = self.accounting.with_statement(STEP_SYMBOL_SQL, |stmt| {
            stmt.query_row(params![id], |r| {
                Ok(StepSymbol {
                    path: r.get(0)?,
                    language: r.get(1)?,
                    kind: r.get(2)?,
                    name: r.get(3)?,
                    top_level: r.get(4)?,
                })
            })
            .optional()
        })?;
        self.symbols.insert(id.to_string(), row.clone());
        Ok(row)
    }
}

impl<'a> Accounting<'a> {
    /// The verdict on a C or C++ site when no definition of the target group is in a file the
    /// site can see.
    pub(super) fn include_reach_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if !is_c_family(&row.language) || spec.group.is_empty() {
            return Ok(None);
        }
        let mut targets = Vec::new();
        for member in &spec.group {
            let path = member.path.replace('\\', "/");
            if !is_c_family(&member.language)
                || !self.outside_includes(&row.path, &path, &member.kind, &member.name)?
            {
                return Ok(None);
            }
            targets.push((path, member.kind.clone()));
        }
        let (target_path, target_kind) = &targets[0];
        let reach = self.site_reach(&row.path)?;
        let named = self.c_named(&spec.name)?;
        let in_reach = |row: &&NamedRow| reach.files.contains(&row.path);
        let definer = named
            .iter()
            .filter(in_reach)
            .find(|named| named.kind == *target_kind && !named.declaration)
            .or_else(|| {
                named
                    .iter()
                    .filter(in_reach)
                    .find(|named| !named.declaration)
            })
            .or_else(|| named.iter().find(in_reach));
        let basis = match definer {
            Some(definer) if definer.path == row.path => format!(
                "this file defines `{}`, and its includes do not reach `{target_path}`",
                spec.name
            ),
            Some(definer) => format!(
                "this file includes `{}` from `{}`, not from `{target_path}`",
                spec.name, definer.path
            ),
            None => format!("this file does not include `{target_path}`"),
        };
        Ok(Some(Verdict::new(SiteLabel::Excluded, basis, false)))
    }

    /// Whether `other`, another definition of the name, is outside the files that a C or C++
    /// `row` can see.
    pub(super) fn include_excludes(
        &mut self,
        row: &NameRow,
        other: &Definition,
    ) -> Result<bool, QueryError> {
        if !is_c_family(&row.language) || !is_c_family(&other.language) {
            return Ok(false);
        }
        self.outside_includes(&row.path, &other.path, &other.kind, &other.name)
    }

    /// Whether the definition at `path` can never be the one a site in `site` names: it is in
    /// no file the site can see, linking cannot bring it in, and no declaration that the site
    /// sees can name it.
    fn outside_includes(
        &mut self,
        site: &str,
        path: &str,
        kind: &str,
        name: &str,
    ) -> Result<bool, QueryError> {
        if path == site
            || matches!(kind, "namespace" | "import" | "export")
            || is_c_source(path) && !is_type_kind(kind)
        {
            return Ok(false);
        }
        let own = self.include_reach(site, false)?;
        if own.files.contains(path) {
            return Ok(false);
        }
        let reach = self.site_reach(site)?;
        if !reach.complete || reach.files.contains(path) {
            return Ok(false);
        }
        if kind != "type" && self.declared_in(&reach, name, kind)? {
            return Ok(false);
        }
        let tag_name = is_type_kind(kind).then(|| last_segment(name).to_string());
        self.text_confirms(&reach, tag_name.as_deref())
    }

    fn site_reach(&mut self, site: &str) -> Result<Rc<IncludeReach>, QueryError> {
        self.include_reach(site, true)
    }

    /// Whether the reach holds a declaration without a body of the same kind and name, which a
    /// definition in another file completes (`int f();`, a method declared in its class).
    fn declared_in(
        &mut self,
        reach: &IncludeReach,
        name: &str,
        kind: &str,
    ) -> Result<bool, QueryError> {
        let mut names = vec![name];
        if last_segment(name) != name {
            names.push(last_segment(name));
        }
        for name in names {
            let named = self.c_named(name)?;
            if named.len() > MAX_NAMED_ROWS
                || named.iter().any(|named| {
                    named.declaration && named.kind == kind && reach.files.contains(&named.path)
                })
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn c_named(&mut self, name: &str) -> Result<Rc<Vec<NamedRow>>, QueryError> {
        if let Some(named) = self.includes.named.get(name) {
            return Ok(named.clone());
        }
        let named = Rc::new(self.with_statement(C_NAMED_SQL, |stmt| {
            stmt.query_map(params![name], |r| {
                Ok(NamedRow {
                    path: r.get(0)?,
                    kind: r.get(1)?,
                    declaration: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()
        })?);
        self.includes.named.insert(name.to_string(), named.clone());
        Ok(named)
    }

    /// Whether the source of every file in the reach agrees with the index: julie recorded each
    /// of its `#include` lines, and, for a type, none of them names it by its tag.
    fn text_confirms(
        &mut self,
        reach: &IncludeReach,
        tag_name: Option<&str>,
    ) -> Result<bool, QueryError> {
        let mut files: Vec<&String> = reach.files.iter().collect();
        files.sort();
        for file in files {
            if self.out_of_time() {
                return Ok(false);
            }
            let recorded = self.includes.includes_recorded.get(file.as_str()).copied();
            let elaborated = tag_name.and_then(|name| {
                self.includes
                    .elaborated
                    .get(&(file.clone(), name.to_string()))
                    .copied()
            });
            if recorded == Some(false) || elaborated == Some(true) {
                return Ok(false);
            }
            if recorded.is_some() && (tag_name.is_none() || elaborated.is_some()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(self.root.join(file)) else {
                return Ok(false);
            };
            let recorded = self.includes_recorded(file, &text)?;
            self.includes
                .includes_recorded
                .insert(file.clone(), recorded);
            let elaborated = tag_name.is_some_and(|name| names_type_by_tag(&text, name));
            if let Some(name) = tag_name {
                self.includes
                    .elaborated
                    .insert((file.clone(), name.to_string()), elaborated);
            }
            if !recorded || elaborated {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Whether julie recorded every `#include` of `file` that can name a project file: an
    /// unrecorded one must name a header that no indexed file matches, and none may name its
    /// header through a macro.
    fn includes_recorded(&mut self, file: &str, text: &str) -> Result<bool, QueryError> {
        let rows = self.load_directives(file)?;
        let recorded: HashSet<&str> = rows
            .iter()
            .filter(|row| row.is_include())
            .map(|row| row.source())
            .collect();
        let dir = file.rfind('/').map_or("", |index| &file[..index]);
        for directive in include_directives(text) {
            let Some((header, system)) = directive else {
                return Ok(false);
            };
            if recorded.contains(header) {
                continue;
            }
            let beside = (!system)
                .then(|| super::loaded_files::join(dir, header))
                .flatten();
            if let Some(beside) = beside
                && self.indexed_file(&beside)?
            {
                return Ok(false);
            }
            if header.starts_with('/')
                || header.starts_with("..")
                || !self
                    .files_by_suffix(header.trim_start_matches("./"))?
                    .is_empty()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The files a site in `site` can see: the file and what it includes, and, with
    /// `with_includers`, every file that includes it, directly or through other headers, with
    /// what those files include.
    fn include_reach(
        &mut self,
        site: &str,
        with_includers: bool,
    ) -> Result<Rc<IncludeReach>, QueryError> {
        let key = (site.to_string(), with_includers);
        if let Some(reach) = self.includes.reach.get(&key) {
            return Ok(reach.clone());
        }
        let mut roots = vec![site.to_string()];
        let mut complete = true;
        if with_includers {
            let (includers, found_all) = self.includers(site)?;
            roots.extend(includers);
            complete = found_all;
        }
        let (files, closed) = self.include_closure(roots)?;
        let reach = Rc::new(IncludeReach {
            files,
            complete: complete && closed,
        });
        self.includes.reach.insert(key, reach.clone());
        Ok(reach)
    }

    /// The files that include `site`, directly or through other files, up to the source files
    /// that no file includes, which a build compiles; incomplete when a header on the way has no
    /// indexed includer.
    fn includers(&mut self, site: &str) -> Result<(Vec<String>, bool), QueryError> {
        let mut seen: HashSet<String> = HashSet::from([site.to_string()]);
        let mut found = Vec::new();
        let mut frontier = vec![site.to_string()];
        let mut complete = true;
        for _ in 0..MAX_INCLUDE_HOPS {
            if frontier.is_empty() {
                break;
            }
            if self.out_of_time() {
                complete = false;
                break;
            }
            let mut names: Vec<&str> = frontier.iter().map(|file| basename(file)).collect();
            names.sort_unstable();
            names.dedup();
            let names = serde_json::to_string(&names).unwrap_or_else(|_| "[]".to_string());
            let candidates = self.with_statement(INCLUDER_FILES_SQL, |stmt| {
                stmt.query_map(params![names], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })?;
            if candidates.len() > MAX_INCLUDERS {
                complete = false;
                break;
            }
            let mut included: HashSet<String> = HashSet::new();
            let mut next = Vec::new();
            for candidate in candidates {
                let rows = self.load_directives(&candidate)?;
                for row in rows.iter().filter(|row| row.is_include()) {
                    if !frontier
                        .iter()
                        .any(|file| basename(file) == basename(row.source()))
                    {
                        continue;
                    }
                    match self.load_reach(&candidate, row)? {
                        Reach::Files(files) => {
                            let hits: Vec<&String> = files
                                .iter()
                                .filter(|file| frontier.contains(file))
                                .collect();
                            if hits.is_empty() {
                                continue;
                            }
                            included.extend(hits.into_iter().cloned());
                            if seen.insert(candidate.clone()) {
                                found.push(candidate.clone());
                                next.push(candidate.clone());
                            }
                        }
                        Reach::Unknown => complete = false,
                        Reach::Outside => {}
                    }
                }
            }
            if frontier
                .iter()
                .any(|file| !is_c_source(file) && !included.contains(file))
                || found.len() > MAX_INCLUDERS
            {
                complete = false;
                break;
            }
            frontier = next;
        }
        if !frontier.is_empty() {
            complete = false;
        }
        Ok((found, complete))
    }

    /// The `roots` and every file they include, directly or through other headers. An include
    /// that several include folders could resolve brings in each candidate.
    fn include_closure(
        &mut self,
        roots: Vec<String>,
    ) -> Result<(HashSet<String>, bool), QueryError> {
        let mut seen: HashSet<String> = roots.iter().cloned().collect();
        let mut frontier = roots;
        let mut complete = true;
        for _ in 0..MAX_INCLUDE_HOPS {
            if frontier.is_empty() {
                break;
            }
            if self.out_of_time() {
                complete = false;
                break;
            }
            let mut next = Vec::new();
            for file in frontier {
                let rows = self.load_directives(&file)?;
                for row in rows.iter().filter(|row| row.is_include()) {
                    match self.load_reach(&file, row)? {
                        Reach::Files(files) => {
                            for path in files {
                                if seen.insert(path.clone()) {
                                    next.push(path);
                                }
                            }
                        }
                        Reach::Outside => {}
                        Reach::Unknown => complete = false,
                    }
                }
            }
            frontier = next;
            if seen.len() > MAX_INCLUDE_FILES {
                complete = false;
                break;
            }
        }
        if !frontier.is_empty() {
            complete = false;
        }
        Ok((seen, complete))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_compiled_extensions_name_a_source_file() {
        assert!(is_c_source("src/main.cpp"));
        assert!(is_c_source("src/util.c"));
        assert!(is_c_source("src/Legacy.C"));
        assert!(!is_c_source("include/a.h"));
        assert!(!is_c_source("include/a.hpp"));
        assert!(!is_c_source("src/a-inl.inc"));
        assert!(!is_c_source("include/vector"));
    }

    #[test]
    fn include_directives_read_include_include_next_import_and_macro_forms() {
        let text = "#include \"a.h\"\n#  include <b/c.h>\n  #include CONFIG_H\n#include_next <c.h>\n#import \"d.h\"\n#define include 1\n#importer x\n";
        assert_eq!(
            include_directives(text),
            vec![
                Some(("a.h", false)),
                Some(("b/c.h", true)),
                None,
                Some(("c.h", true)),
                Some(("d.h", false)),
            ]
        );
    }

    #[test]
    fn a_tag_without_a_body_names_a_type_and_a_definition_does_not() {
        assert!(names_type_by_tag("struct S;\n", "S"));
        assert!(names_type_by_tag("void f(struct S *p);\n", "S"));
        assert!(names_type_by_tag("friend class S;\n", "S"));
        assert!(names_type_by_tag("class API_EXPORT S;\n", "S"));
        assert!(!names_type_by_tag("struct S {\n};\n", "S"));
        assert!(!names_type_by_tag("class S : public B {};\n", "S"));
        assert!(!names_type_by_tag("class S final {};\n", "S"));
        assert!(!names_type_by_tag("friend class ::ns::detail::S;\n", "S"));
        assert!(!names_type_by_tag("struct SS;\nS x;\n", "S"));
    }
}
