//! The files and namespaces that the imports of a file open for a related-type name.
//!
//! Python imports, Rust `use` paths, and C and C++ quoted includes name files; C# and Razor
//! usings and enclosing namespaces name namespaces. The owner query binds a type only when one
//! candidate remains, so a name that two of these reach stays unresolved.

use std::collections::{HashMap, HashSet};

use rusqlite::{Connection, OptionalExtension, params};

use super::{QueryError, RELATED_TYPE_KINDS};

const MAX_IMPORT_HOPS: usize = 4;
const MAX_INCLUDED_FILES: usize = 64;
const USING_RANK: usize = 1000;

/// JSON arrays for the owner query: `files` holds `[path, type name]` pairs, `namespaces`
/// holds `[namespace, rank]` pairs where a lower rank shadows a higher one.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct ImportScope {
    pub files: String,
    pub namespaces: String,
}

/// Reads import scopes for the type facts of one context, and keeps each file's scope and
/// the `global using` namespaces for the facts that follow.
pub(super) struct ScopeReader<'c> {
    conn: &'c Connection,
    files: HashMap<(String, String), String>,
    namespaces: HashMap<(String, Option<String>), String>,
    global_usings: Option<Vec<String>>,
}

impl<'c> ScopeReader<'c> {
    pub(super) fn new(conn: &'c Connection) -> Self {
        Self {
            conn,
            files: HashMap::new(),
            namespaces: HashMap::new(),
            global_usings: None,
        }
    }

    /// The import scope of the type name `resolved_type` written in the symbol `source_id`.
    pub(super) fn scope(
        &mut self,
        source_id: &str,
        source_path: &str,
        language: &str,
        resolved_type: &str,
    ) -> Result<ImportScope, QueryError> {
        let conn = self.conn;
        let file_key = (source_path.to_string(), resolved_type.to_string());
        if !self.files.contains_key(&file_key) {
            let files = match language {
                "python" => {
                    imported_definitions(conn, source_path, resolved_type, python_import_files)?
                }
                "rust" => imported_definitions(conn, source_path, resolved_type, rust_use_files)?,
                "c" | "cpp" => included_files(conn, source_path)?
                    .into_iter()
                    .map(|path| (path, resolved_type.to_string()))
                    .collect(),
                _ => Vec::new(),
            };
            self.files.insert(file_key.clone(), to_json(&files));
        }
        let enclosing = match language {
            "csharp" => enclosing_namespace(conn, source_id, source_path)?,
            _ => None,
        };
        let namespace_key = (source_path.to_string(), enclosing.clone());
        if !self.namespaces.contains_key(&namespace_key) {
            let namespaces = match language {
                "csharp" => {
                    let mut namespaces =
                        csharp_namespaces(conn, enclosing.as_deref(), source_path)?;
                    namespaces.extend(
                        self.global_usings()?
                            .iter()
                            .map(|namespace| (namespace.clone(), USING_RANK)),
                    );
                    namespaces
                }
                "razor" => razor_namespaces(conn, source_path)?,
                _ => Vec::new(),
            };
            self.namespaces
                .insert(namespace_key.clone(), to_json(&namespaces));
        }
        Ok(ImportScope {
            files: self.files[&file_key].clone(),
            namespaces: self.namespaces[&namespace_key].clone(),
        })
    }

    fn global_usings(&mut self) -> Result<&[String], QueryError> {
        if self.global_usings.is_none() {
            let mut stmt = self.conn.prepare(
                "SELECT COALESCE(signature, '') FROM symbols
                 WHERE kind = 'import' AND language = 'csharp'
                   AND signature GLOB 'global using *'",
            )?;
            let mut namespaces = Vec::new();
            for signature in stmt.query_map([], |row| row.get::<_, String>(0))? {
                namespaces.extend(using_namespace(&signature?));
            }
            self.global_usings = Some(namespaces);
        }
        Ok(self.global_usings.as_deref().unwrap_or_default())
    }
}

fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "[]".to_string())
}

struct ImportRow {
    signature: String,
    source: String,
    imported: String,
}

type ImportFiles = fn(&Connection, &str, &str, &ImportRow) -> Result<Vec<String>, QueryError>;

/// The `(file, name)` definitions that the imports of `binding` in `path` reach, following a
/// file that imports the name again instead of defining it (`from .ctx import X` in a package
/// `__init__.py`, `pub use models::Symbol;`).
fn imported_definitions(
    conn: &Connection,
    path: &str,
    binding: &str,
    files_of: ImportFiles,
) -> Result<Vec<(String, String)>, QueryError> {
    let mut found = Vec::new();
    let mut seen = HashSet::new();
    let mut frontier = vec![(path.to_string(), binding.to_string())];
    for _ in 0..MAX_IMPORT_HOPS {
        let mut next = Vec::new();
        for (file, name) in frontier {
            for row in import_rows(conn, &file, &name)? {
                for target in files_of(conn, &file, &name, &row)? {
                    if !seen.insert((target.clone(), row.imported.clone())) {
                        continue;
                    }
                    if defines_type(conn, &target, &row.imported)? {
                        found.push((target, row.imported.clone()));
                    } else {
                        next.push((target, row.imported.clone()));
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(found)
}

fn import_rows(conn: &Connection, path: &str, name: &str) -> Result<Vec<ImportRow>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT COALESCE(signature, ''),
                CASE WHEN json_valid(metadata_json)
                     THEN COALESCE(json_extract(metadata_json, '$.source'), '') ELSE '' END,
                CASE WHEN json_valid(metadata_json)
                     THEN COALESCE(NULLIF(json_extract(metadata_json, '$.importedName'), ''), name)
                     ELSE name END
         FROM symbols WHERE kind = 'import' AND path = ?1 AND name = ?2",
    )?;
    let rows = stmt.query_map(params![path, name], |row| {
        Ok(ImportRow {
            signature: row.get(0)?,
            source: row.get(1)?,
            imported: row.get(2)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn defines_type(conn: &Connection, path: &str, name: &str) -> Result<bool, QueryError> {
    let mut stmt = conn.prepare(&format!(
        "SELECT 1 FROM symbols WHERE path = ?1 AND name = ?2
            AND kind IN ({RELATED_TYPE_KINDS}) AND {} LIMIT 1",
        super::module_level_type_sql("symbols")
    ))?;
    Ok(stmt.exists(params![path, name])?)
}

fn indexed(conn: &Connection, path: &str) -> Result<bool, QueryError> {
    let mut stmt = conn.prepare("SELECT 1 FROM files WHERE path = ?1")?;
    Ok(stmt.exists(params![path])?)
}

fn indexed_of(conn: &Connection, candidates: Vec<String>) -> Result<Vec<String>, QueryError> {
    let mut files = Vec::new();
    for candidate in candidates {
        if indexed(conn, &candidate)? {
            files.push(candidate);
        }
    }
    Ok(files)
}

fn parent_dir(path: &str) -> &str {
    path.rfind('/').map_or("", |index| &path[..index])
}

fn join(dir: &str, rest: &str) -> String {
    if dir.is_empty() {
        rest.to_string()
    } else {
        format!("{dir}/{rest}")
    }
}

/// The module files of a Python import source: `.ctx` and `..ctx` from the importer's
/// package, and `pkg.sub.ctx` at any folder that holds `pkg/sub/ctx.py`.
fn python_import_files(
    conn: &Connection,
    path: &str,
    _binding: &str,
    row: &ImportRow,
) -> Result<Vec<String>, QueryError> {
    let level = row.source.chars().take_while(|c| *c == '.').count();
    let module = row.source[level..].replace('.', "/");
    if level > 0 {
        let mut base = parent_dir(path);
        for _ in 1..level {
            if base.is_empty() {
                return Ok(Vec::new());
            }
            base = parent_dir(base);
        }
        let candidates = if module.is_empty() {
            vec![join(base, "__init__.py")]
        } else {
            vec![
                join(base, &format!("{module}.py")),
                join(base, &format!("{module}/__init__.py")),
            ]
        };
        return indexed_of(conn, candidates);
    }
    if module.is_empty() || module.contains(['*', '?', '[']) {
        return Ok(Vec::new());
    }
    let mut stmt = conn.prepare(
        "SELECT path FROM files
         WHERE path = ?1 || '.py' OR path = ?1 || '/__init__.py'
            OR path GLOB '*/' || ?1 || '.py' OR path GLOB '*/' || ?1 || '/__init__.py'
         ORDER BY path",
    )?;
    let files = stmt.query_map(params![module], |row| row.get(0))?;
    Ok(files.collect::<Result<_, _>>()?)
}

/// The module file that a Rust `use` path names: `crate::`, `self::`, and `super::` from the
/// importer's module, a crate name (`my_core::`) at the package folder `my-core/src`, and a
/// name of a module beside the importer. `std`, `core`, `alloc`, and other crates name none.
fn rust_use_files(
    conn: &Connection,
    path: &str,
    binding: &str,
    row: &ImportRow,
) -> Result<Vec<String>, QueryError> {
    let Some(segments) = rust_use_paths(&row.signature)
        .into_iter()
        .find_map(|(segments, bound)| (bound == binding).then_some(segments))
    else {
        return Ok(Vec::new());
    };
    let Some((_, module)) = segments.split_last() else {
        return Ok(Vec::new());
    };
    let Some(src) = crate_src_dir(path) else {
        return Ok(Vec::new());
    };
    let file_name = &path[path.rfind('/').map_or(0, |index| index + 1)..];
    let stem = file_name.strip_suffix(".rs").unwrap_or(file_name);
    let current = if matches!(stem, "mod" | "lib" | "main") {
        parent_dir(path).to_string()
    } else {
        join(parent_dir(path), stem)
    };
    let (mut dir, rest) = match module.first().map(String::as_str) {
        None => (current, module),
        Some("crate") => (src.to_string(), &module[1..]),
        Some("self" | "super") => {
            let mut dir = current;
            let supers = module
                .iter()
                .take_while(|segment| matches!(segment.as_str(), "self" | "super"))
                .collect::<Vec<_>>();
            for segment in &supers {
                if segment.as_str() == "super" {
                    if dir == src || !dir.starts_with(src) {
                        return Ok(Vec::new());
                    }
                    dir = parent_dir(&dir).to_string();
                }
            }
            (dir, &module[supers.len()..])
        }
        Some("std" | "core" | "alloc") => return Ok(Vec::new()),
        Some(first) => match package_src_dir(conn, first)? {
            Some(package) => (package, &module[1..]),
            None => (current, module),
        },
    };
    for segment in rest {
        dir = join(&dir, segment);
    }
    let candidates = if dir == src || dir.ends_with("/src") || dir == "src" {
        vec![join(&dir, "lib.rs"), join(&dir, "main.rs")]
    } else {
        vec![format!("{dir}.rs"), join(&dir, "mod.rs")]
    };
    indexed_of(conn, candidates)
}

fn crate_src_dir(path: &str) -> Option<&str> {
    if path.starts_with("src/") {
        return Some("src");
    }
    path.rfind("/src/").map(|index| &path[..index + 4])
}

/// The `src` folder of the package whose folder name, with `-` read as `_`, is `crate_name`.
fn package_src_dir(conn: &Connection, crate_name: &str) -> Result<Option<String>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT path FROM files WHERE path GLOB '*src/lib.rs' ORDER BY length(path), path",
    )?;
    let roots = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut matches = Vec::new();
    for root in roots {
        let root = root?;
        let src = parent_dir(&root);
        let package = parent_dir(src);
        let folder = &package[package.rfind('/').map_or(0, |index| index + 1)..];
        if !folder.is_empty() && folder.replace('-', "_") == crate_name {
            matches.push(src.to_string());
        }
    }
    Ok((matches.len() == 1).then(|| matches.remove(0)))
}

/// Each path of a `use` tree with the name it binds: `use a::{b::C, D as E};` gives
/// `([a, b, C], C)` and `([a, D], E)`. A glob binds no name.
fn rust_use_paths(signature: &str) -> Vec<(Vec<String>, String)> {
    let mut text = signature.trim();
    if let Some(rest) = text.strip_prefix("pub") {
        text = rest.trim_start();
        if text.starts_with('(') {
            text = text
                .find(')')
                .map_or("", |index| &text[index + 1..])
                .trim_start();
        }
    }
    let Some(tree) = text.strip_prefix("use") else {
        return Vec::new();
    };
    let tokens = rust_use_tokens(tree.trim().trim_end_matches(';'));
    let mut paths = Vec::new();
    let mut cursor = 0;
    parse_use_tree(&tokens, &mut cursor, &[], &mut paths);
    paths
}

fn rust_use_tokens(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            ':' if chars.peek() == Some(&':') => {
                chars.next();
                tokens.push("::".to_string());
            }
            '{' | '}' | ',' | '*' => tokens.push(c.to_string()),
            c if c.is_alphanumeric() || c == '_' || c == '#' => {
                let mut word = c.to_string();
                while let Some(&next) = chars.peek() {
                    if next.is_alphanumeric() || next == '_' || next == '#' {
                        word.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(word.trim_start_matches("r#").to_string());
            }
            _ => {}
        }
    }
    tokens
}

fn parse_use_tree(
    tokens: &[String],
    cursor: &mut usize,
    prefix: &[String],
    paths: &mut Vec<(Vec<String>, String)>,
) {
    let mut segments = prefix.to_vec();
    while let Some(token) = tokens.get(*cursor) {
        match token.as_str() {
            "{" => {
                *cursor += 1;
                while tokens.get(*cursor).is_some_and(|token| token != "}") {
                    parse_use_tree(tokens, cursor, &segments, paths);
                    if tokens.get(*cursor).is_some_and(|token| token == ",") {
                        *cursor += 1;
                    }
                }
                *cursor += 1;
                return;
            }
            "*" => {
                *cursor += 1;
                return;
            }
            "," | "}" | "::" => return,
            word => {
                segments.push(word.to_string());
                *cursor += 1;
                if tokens.get(*cursor).is_some_and(|token| token == "::") {
                    *cursor += 1;
                    continue;
                }
                let mut bound = word.to_string();
                if tokens.get(*cursor).is_some_and(|token| token == "as")
                    && let Some(alias) = tokens.get(*cursor + 1)
                {
                    bound = alias.clone();
                    *cursor += 2;
                }
                paths.push((segments, bound));
                return;
            }
        }
    }
}

/// The project files that the includes of `path` reach, and the includes of those files, up
/// to four steps and 64 files. A quoted include resolves from the includer's folder, else at
/// any folder that holds that relative path, as an include path would; an angle-bracket
/// include resolves only at such a folder (`<nlohmann/detail/input/position_t.hpp>`).
fn included_files(conn: &Connection, path: &str) -> Result<Vec<String>, QueryError> {
    let mut stmt = conn.prepare(
        "SELECT json_extract(metadata_json, '$.path'),
                COALESCE(json_extract(metadata_json, '$.isSystemHeader'), 'false') IN ('true', 1)
         FROM symbols
         WHERE kind = 'import' AND path = ?1 AND json_valid(metadata_json)
           AND json_extract(metadata_json, '$.type') = 'include'
           AND json_extract(metadata_json, '$.path') IS NOT NULL",
    )?;
    let mut by_suffix = conn.prepare(
        "SELECT path FROM files WHERE path = ?1 OR path GLOB '*/' || ?1 ORDER BY path LIMIT 8",
    )?;
    let mut seen = HashSet::from([path.to_string()]);
    let mut files = Vec::new();
    let mut frontier = vec![path.to_string()];
    for _ in 0..MAX_IMPORT_HOPS {
        let mut next = Vec::new();
        for file in frontier {
            let includes = stmt
                .query_map(params![file], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (include, system) in includes {
                let include = include.replace('\\', "/");
                if include.contains(['*', '?', '[']) {
                    continue;
                }
                let beside = (!system)
                    .then(|| normalize(&join(parent_dir(&file), &include)))
                    .flatten();
                let targets = match beside {
                    Some(beside) if indexed(conn, &beside)? => vec![beside],
                    _ => by_suffix
                        .query_map(params![include.trim_start_matches("./")], |row| row.get(0))?
                        .collect::<Result<Vec<String>, _>>()?,
                };
                for target in targets {
                    if files.len() >= MAX_INCLUDED_FILES {
                        return Ok(files);
                    }
                    if seen.insert(target.clone()) {
                        files.push(target.clone());
                        next.push(target);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    Ok(files)
}

fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
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

/// The namespace that holds a C# symbol, or the one namespace of its file.
fn enclosing_namespace(
    conn: &Connection,
    source_id: &str,
    path: &str,
) -> Result<Option<String>, QueryError> {
    let enclosing: Option<String> = conn
        .query_row(
            "WITH RECURSIVE up(symbol_id, parent_symbol_id, kind, name, depth) AS (
                 SELECT symbol_id, parent_symbol_id, kind, name, 0 FROM symbols WHERE symbol_id = ?1
                 UNION ALL
                 SELECT s.symbol_id, s.parent_symbol_id, s.kind, s.name, up.depth + 1
                 FROM symbols s JOIN up ON s.symbol_id = up.parent_symbol_id
                 WHERE up.depth < 32
             )
             SELECT name FROM up WHERE kind = 'namespace' ORDER BY depth LIMIT 1",
            params![source_id],
            |row| row.get(0),
        )
        .optional()?;
    let enclosing = match enclosing {
        Some(name) => Some(name),
        None => {
            let mut stmt = conn.prepare(
                "SELECT name FROM symbols WHERE path = ?1 AND kind = 'namespace' LIMIT 2",
            )?;
            let names = stmt
                .query_map(params![path], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            (names.len() == 1).then(|| names[0].clone())
        }
    };
    Ok(enclosing)
}

/// The namespaces a C# symbol sees from its file: its namespace and each one around it
/// (`App.Rendering`, then `App`), then the namespaces of the file's usings.
fn csharp_namespaces(
    conn: &Connection,
    enclosing: Option<&str>,
    path: &str,
) -> Result<Vec<(String, usize)>, QueryError> {
    let mut namespaces = Vec::new();
    if let Some(name) = enclosing {
        let mut current = name;
        let mut rank = 0;
        loop {
            namespaces.push((current.to_string(), rank));
            rank += 1;
            match current.rfind('.') {
                Some(index) => current = &current[..index],
                None => break,
            }
        }
    }
    let mut stmt = conn.prepare(
        "SELECT COALESCE(signature, '') FROM symbols
         WHERE kind = 'import' AND language = 'csharp' AND path = ?1",
    )?;
    for signature in stmt.query_map(params![path], |row| row.get::<_, String>(0))? {
        if let Some(namespace) = using_namespace(&signature?) {
            namespaces.push((namespace, USING_RANK));
        }
    }
    Ok(namespaces)
}

/// The namespaces of a Razor file's `@using` lines and of the `_Imports.razor` files in its
/// folder and the folders above it.
fn razor_namespaces(conn: &Connection, path: &str) -> Result<Vec<(String, usize)>, QueryError> {
    let mut files = vec![path.to_string()];
    let mut dir = parent_dir(path);
    loop {
        files.push(join(dir, "_Imports.razor"));
        if dir.is_empty() {
            break;
        }
        dir = parent_dir(dir);
    }
    let mut stmt = conn.prepare(
        "SELECT COALESCE(signature, '') FROM symbols WHERE kind = 'import' AND path = ?1",
    )?;
    let mut namespaces = Vec::new();
    for file in files {
        for signature in stmt.query_map(params![file], |row| row.get::<_, String>(0))? {
            if let Some(namespace) = using_namespace(&signature?) {
                namespaces.push((namespace, USING_RANK));
            }
        }
    }
    Ok(namespaces)
}

/// The namespace of `using App.Data;`, `@using App.Data`, or `global using App.Data;`. An
/// alias (`using Name = …;`) and `using static` open no namespace.
fn using_namespace(signature: &str) -> Option<String> {
    let text = signature.trim().trim_start_matches('@');
    let text = text.strip_prefix("global").map_or(text, str::trim_start);
    let namespace = text
        .strip_prefix("using")?
        .trim()
        .trim_end_matches(';')
        .trim();
    let simple = !namespace.is_empty()
        && !namespace.starts_with("static ")
        && namespace
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '.');
    simple.then(|| namespace.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_trees_name_each_path_with_its_binding() {
        let paths = rust_use_paths(
            "pub(crate) use crate::{db::Connection, models::{Symbol as Sym, Kind}, util::*};",
        );
        let named: Vec<(String, String)> = paths
            .into_iter()
            .map(|(segments, bound)| (segments.join("::"), bound))
            .collect();
        assert_eq!(
            named,
            [
                (
                    "crate::db::Connection".to_string(),
                    "Connection".to_string()
                ),
                ("crate::models::Symbol".to_string(), "Sym".to_string()),
                ("crate::models::Kind".to_string(), "Kind".to_string()),
            ]
        );
        assert_eq!(rust_use_paths("let x = 1;"), Vec::new());
    }

    #[test]
    fn usings_open_only_plain_namespaces() {
        assert_eq!(
            using_namespace("using App.Data"),
            Some("App.Data".to_string())
        );
        assert_eq!(
            using_namespace("@using App.Data"),
            Some("App.Data".to_string())
        );
        assert_eq!(
            using_namespace("global using App;"),
            Some("App".to_string())
        );
        assert_eq!(using_namespace("using static App.Math;"), None);
        assert_eq!(using_namespace("using Json = Newtonsoft.Json;"), None);
    }

    #[test]
    fn include_paths_normalize_inside_the_project_only() {
        assert_eq!(normalize("src/./a/../jv.h"), Some("src/jv.h".to_string()));
        assert_eq!(normalize("../jv.h"), None);
    }

    #[test]
    fn crate_sources_start_at_the_nearest_src_folder() {
        assert_eq!(
            crate_src_dir("crates/core/src/report/arity.rs"),
            Some("crates/core/src")
        );
        assert_eq!(crate_src_dir("src/lib.rs"), Some("src"));
        assert_eq!(crate_src_dir("build.rs"), None);
    }
}
