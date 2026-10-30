//! The project files that an import, include, or load names by its path, and the names those
//! files bring into a file.
//!
//! A bare call in Bash, PowerShell, R, PHP, or Ruby reaches a top-level function that the
//! caller's file loads (`source`, dot-source, `source()`, `require`, `require_relative`). A C or
//! C++ call sees the declarations of the headers its file includes. A Dart import prefix
//! (`import 'x.dart' as s;`) and a GDScript `preload` constant reach only the files they name.
//! A path that depends on runtime state (`$DIR/env.sh`, a PowerShell module name) is unknown and
//! proves nothing.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use rusqlite::{Row, params};

use super::{
    Accounting, Definition, FILE_EXISTS_SQL, IMPORT_BASIS, NameRow, QueryError, SELF_RECEIVERS,
    SiteLabel, Spec, Verdict, is_plain_identifier,
};
use crate::models::Symbol;

const MAX_LOAD_HOPS: usize = 4;
const MAX_LOADED_FILES: usize = 64;
const MAX_BASE_HOPS: usize = 8;
/// An include that more indexed files can resolve than this names an unknown file.
const MAX_INCLUDE_CANDIDATES: usize = 8;
/// At this many rows that could load a file, the file counts as loaded by another one, unchecked.
const MAX_LOADER_ROWS: usize = 200;

const LOAD_ROWS_SQL: &str = "SELECT start_line, name, kind, language,
            COALESCE(json_extract(metadata_json, '$.source'), ''),
            COALESCE(json_extract(metadata_json, '$.form'), ''),
            COALESCE(json_extract(metadata_json, '$.relative_to'), ''),
            COALESCE(json_extract(metadata_json, '$.alias'), ''),
            COALESCE(json_extract(metadata_json, '$.isSystemHeader'), 'false') IN ('true', 1),
            COALESCE(json_extract(metadata_json, '$.show'), '[]'),
            COALESCE(json_extract(metadata_json, '$.hide'), '[]'),
            COALESCE(signature, '') GLOB '* if*(*'
     FROM symbols
     WHERE path = ?1 AND kind IN ('import', 'export') AND json_valid(metadata_json)
     ORDER BY start_line";

const LOADER_ROWS_SQL: &str = "SELECT path FROM symbols
     WHERE kind = 'import' AND language = ?1 AND json_valid(metadata_json)
       AND (json_extract(metadata_json, '$.source') LIKE ?2 ESCAPE '\\' OR name LIKE ?2 ESCAPE '\\')
     LIMIT 201";

const DEFINES_TOP_LEVEL_SQL: &str = "SELECT 1 FROM symbols
     WHERE path = ?1 AND name = ?2 AND parent_symbol_id IS NULL
       AND kind NOT IN ('import', 'export') LIMIT 1";

const SUFFIX_FILES_SQL: &str = "SELECT replace(path, '\\', '/') FROM files
     WHERE replace(path, '\\', '/') = ?1 OR replace(path, '\\', '/') GLOB '*/' || ?1
     ORDER BY path LIMIT 9";

const PUBSPECS_SQL: &str = "SELECT replace(path, '\\', '/') FROM files
     WHERE path = 'pubspec.yaml' OR replace(path, '\\', '/') LIKE '%/pubspec.yaml'
     LIMIT 200";

const FILE_CLASS_SQL: &str = "SELECT name,
            COALESCE(json_extract(metadata_json, '$.baseClass'), '')
     FROM symbols
     WHERE path = ?1 AND language = 'gdscript' AND kind = 'class' AND parent_symbol_id IS NULL";

const NAMED_GDSCRIPT_CLASS_SQL: &str = "SELECT replace(path, '\\', '/') FROM symbols
     WHERE name = ?1 AND language = 'gdscript' AND kind = 'class' AND parent_symbol_id IS NULL
       AND signature LIKE 'class_name%'
     LIMIT 2";

const FILE_CLASS_MEMBER_SQL: &str = "SELECT 1 FROM symbols m
     JOIN symbols c ON c.symbol_id = m.parent_symbol_id
     WHERE m.path = ?1 AND m.name = ?2 AND c.parent_symbol_id IS NULL AND c.kind = 'class'
     LIMIT 1";

const OWNED_METHOD_SQL: &str = "SELECT 1 FROM symbols
     WHERE name = ?1 AND language = ?2 AND parent_symbol_id IS NOT NULL
       AND kind IN ('method', 'function') LIMIT 1";

const LOCAL_NAMED_SQL: &str = "SELECT 1 FROM symbols
     WHERE path = ?1 AND name = ?2 AND kind NOT IN ('import', 'export') LIMIT 1";

const PHP_NAMESPACE_SQL: &str =
    "SELECT 1 FROM symbols WHERE path = ?1 AND language = 'php' AND kind = 'namespace' LIMIT 1";

const CPP_NAMESPACE_SQL: &str = "SELECT 1 FROM symbols
     WHERE name = ?1 AND kind = 'namespace' AND language IN ('cpp', 'c') LIMIT 1";

/// One import, include, load, or export row of a file.
#[derive(Clone, Debug)]
pub(super) struct LoadRow {
    line: usize,
    name: String,
    kind: String,
    language: String,
    source: String,
    form: String,
    relative_to: String,
    alias: String,
    system: bool,
    show: Vec<String>,
    hide: Vec<String>,
    /// A Dart conditional import (`import 'a.dart' if (dart.library.io) 'b.dart';`), whose other
    /// file the row does not name.
    conditional: bool,
}

impl LoadRow {
    fn read(row: &Row) -> rusqlite::Result<Self> {
        let list = |text: String| serde_json::from_str::<Vec<String>>(&text).unwrap_or_default();
        Ok(LoadRow {
            line: row.get::<_, i64>(0)? as usize,
            name: row.get(1)?,
            kind: row.get(2)?,
            language: row.get(3)?,
            source: row.get(4)?,
            form: row.get(5)?,
            relative_to: row.get(6)?,
            alias: row.get(7)?,
            system: row.get(8)?,
            show: list(row.get(9)?),
            hide: list(row.get(10)?),
            conditional: row.get(11)?,
        })
    }

    /// Whether the row is a C or C++ `#include`.
    pub(super) fn is_include(&self) -> bool {
        matches!(self.language.as_str(), "c" | "cpp") && self.form == "#include"
    }

    pub(super) fn source(&self) -> &str {
        &self.source
    }

    /// Whether the row brings `name` in: a Dart `show` list that leaves it out, or a `hide`
    /// list that names it, keeps it out.
    fn admits(&self, name: &str) -> bool {
        (self.show.is_empty() || self.show.iter().any(|shown| shown == name))
            && !self.hide.iter().any(|hidden| hidden == name)
    }
}

/// Where an import source leads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Reach {
    /// The indexed project files it names; one for a load, more for a C include that several
    /// include folders could hold.
    Files(Vec<String>),
    /// No project file: a runtime library, a system header, or a package outside the project.
    Outside,
    /// The index cannot tell: a path built from runtime state, a module search path, or a file
    /// that is not indexed.
    Unknown,
}

/// The project files that a file loads, directly or through the files it loads.
pub(super) struct LoadScope {
    /// Each loaded file with the line of the caller file's own load row that leads to it.
    files: Vec<(String, usize)>,
    /// Every load on the way was resolved, and the walk stayed within its bounds.
    complete: bool,
}

#[derive(Default)]
pub(super) struct LoadCaches {
    rows: HashMap<String, Rc<Vec<LoadRow>>>,
    reach: HashMap<String, Reach>,
    scopes: HashMap<String, Rc<LoadScope>>,
    loaded_elsewhere: HashMap<String, bool>,
    defines: HashMap<(String, String), bool>,
    pubspecs: Option<Rc<Vec<(String, String)>>>,
    r_packages: HashMap<String, Option<String>>,
    suffix_files: HashMap<String, Vec<String>>,
}

/// Whether an import row binds its name or alias as a receiver in code (`const Helper =
/// preload(…)`, `import 'x.dart' as s`, `namespace fs = std::filesystem`). A C or C++ `#include`,
/// a CSS `@import`, a Bash `source`, a PowerShell dot-source or `Import-Module`, an R
/// `source()`, a Ruby `require`, a PHP `require` or `include`, an F# `open` or `#load`, and a
/// Java star import load a file or open its names without binding one.
/// [`crate::queries::import_binds_name_sql`] is the same rule in SQL.
pub(super) fn binds_name(language: &str, form: &str, wildcard: bool) -> bool {
    if wildcard {
        return false;
    }
    match language {
        "c" | "css" | "bash" | "powershell" | "fsharp" => false,
        "cpp" => matches!(form, "namespace" | "using" | "using namespace"),
        "ruby" => form == "autoload",
        "r" => !matches!(form, "source()" | ""),
        "php" => !is_php_include(form),
        _ => true,
    }
}

/// The languages whose import sources this module resolves.
pub(super) fn resolves_sources(language: &str) -> bool {
    matches!(
        language,
        "c" | "cpp"
            | "css"
            | "bash"
            | "powershell"
            | "r"
            | "ruby"
            | "fsharp"
            | "dart"
            | "php"
            | "gdscript"
    )
}

fn is_php_include(form: &str) -> bool {
    matches!(
        form,
        "require" | "require_once" | "include" | "include_once"
    )
}

/// The languages whose bare calls reach the top-level functions of the files a file loads, which
/// go into one global scope.
fn loads_into_global_scope(language: &str) -> bool {
    matches!(language, "bash" | "powershell" | "r" | "php" | "ruby")
}

/// Whether `row` loads code into the file that holds it.
fn is_load(row: &LoadRow) -> bool {
    match row.language.as_str() {
        "c" | "cpp" => row.form == "#include",
        "bash" => matches!(
            row.form.as_str(),
            "source" | "." | "load" | "Include" | "bats_load_library"
        ),
        "powershell" => matches!(row.form.as_str(), "." | "Import-Module" | "using module"),
        "r" => matches!(
            row.form.as_str(),
            "source()" | "library()" | "require()" | "box::use()" | "import::from()"
        ),
        "ruby" => matches!(row.form.as_str(), "require" | "require_relative" | "load"),
        "php" => is_php_include(&row.form),
        _ => false,
    }
}

fn parent_dir(path: &str) -> &str {
    path.rfind('/').map_or("", |index| &path[..index])
}

/// `rest` read from the folder `dir`, with `.` and `name/..` segments removed; `None` when it
/// climbs above the project root.
pub(super) fn join(dir: &str, rest: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|part| !part.is_empty()).collect();
    for part in rest.split('/') {
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

fn has_extension(path: &str) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| name.rfind('.').is_some_and(|dot| dot > 0))
}

fn stem(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rfind('.')
        .filter(|dot| *dot > 0)
        .map_or(name, |dot| &name[..dot])
}

/// Whether a C or C++ signature declares a function with internal linkage.
fn is_static_signature(signature: &str) -> bool {
    signature
        .split('(')
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .any(|word| word == "static")
}

fn display_list(paths: &[String]) -> String {
    paths
        .first()
        .map(|first| format!("`{first}`"))
        .unwrap_or_default()
}

impl<'a> Accounting<'a> {
    pub(super) fn load_directives(&mut self, path: &str) -> Result<Rc<Vec<LoadRow>>, QueryError> {
        if let Some(rows) = self.loads.rows.get(path) {
            return Ok(rows.clone());
        }
        let rows = Rc::new(self.rows_at_path(LOAD_ROWS_SQL, path, &[], LoadRow::read)?);
        self.loads.rows.insert(path.to_string(), rows.clone());
        Ok(rows)
    }

    pub(super) fn indexed_file(&self, path: &str) -> Result<bool, QueryError> {
        let windows = path.replace('/', "\\");
        self.with_statement(FILE_EXISTS_SQL, |stmt| stmt.exists(params![path, windows]))
    }

    /// The candidates that are indexed files, once each in order.
    fn indexed_of(&self, candidates: &[Option<String>]) -> Result<Vec<String>, QueryError> {
        let mut found: Vec<String> = Vec::new();
        for candidate in candidates.iter().flatten() {
            if !candidate.is_empty()
                && !found.contains(candidate)
                && self.indexed_file(candidate)?
            {
                found.push(candidate.clone());
            }
        }
        Ok(found)
    }

    /// One file from the candidates, unknown when none or two different ones are indexed.
    fn one_of(&self, candidates: &[Option<String>]) -> Result<Reach, QueryError> {
        Ok(match self.indexed_of(candidates)?.as_slice() {
            [only] => Reach::Files(vec![only.clone()]),
            _ => Reach::Unknown,
        })
    }

    /// The indexed files whose path ends with `suffix` at a folder boundary, as an include
    /// folder or a load path would find them.
    pub(super) fn files_by_suffix(&mut self, suffix: &str) -> Result<Vec<String>, QueryError> {
        if let Some(found) = self.loads.suffix_files.get(suffix) {
            return Ok(found.clone());
        }
        let found = self.with_statement(SUFFIX_FILES_SQL, |stmt| {
            stmt.query_map(params![suffix], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        self.loads
            .suffix_files
            .insert(suffix.to_string(), found.clone());
        Ok(found)
    }

    /// Where the import `row` of the file `importer` leads.
    pub(super) fn load_reach(
        &mut self,
        importer: &str,
        row: &LoadRow,
    ) -> Result<Reach, QueryError> {
        let key = format!(
            "{importer}\0{}\0{}\0{}\0{}\0{}",
            row.language, row.form, row.source, row.relative_to, row.system
        );
        if let Some(reach) = self.loads.reach.get(&key) {
            return Ok(reach.clone());
        }
        let reach = self.resolve(importer, row)?;
        self.loads.reach.insert(key, reach.clone());
        Ok(reach)
    }

    fn resolve(&mut self, importer: &str, row: &LoadRow) -> Result<Reach, QueryError> {
        let source = row.source.trim().replace('\\', "/");
        if source.is_empty() {
            return Ok(Reach::Unknown);
        }
        let dir = parent_dir(importer);
        let runtime_state = source.contains(['$', '`', '*', '?']) || source.starts_with('~');
        match row.language.as_str() {
            "c" | "cpp" if row.form == "#include" => {
                let beside = (!row.system).then(|| join(dir, &source)).flatten();
                if let Some(beside) = beside
                    && self.indexed_file(&beside)?
                {
                    return Ok(Reach::Files(vec![beside]));
                }
                if source.starts_with('/') || source.starts_with("..") {
                    return Ok(Reach::Unknown);
                }
                let found = self.files_by_suffix(source.trim_start_matches("./"))?;
                Ok(if found.is_empty() {
                    Reach::Outside
                } else if found.len() > MAX_INCLUDE_CANDIDATES {
                    Reach::Unknown
                } else {
                    Reach::Files(found)
                })
            }
            "cpp" => self.cpp_namespace_reach(&source),
            "css" => {
                if source.contains("://") || source.starts_with("//") {
                    Ok(Reach::Outside)
                } else if source.starts_with('/') {
                    Ok(Reach::Unknown)
                } else {
                    self.one_of(&[join(dir, &source)])
                }
            }
            "bash" => {
                if runtime_state || source.starts_with('/') {
                    return Ok(Reach::Unknown);
                }
                match row.form.as_str() {
                    "load" => {
                        self.one_of(&[join(dir, &format!("{source}.bash")), join(dir, &source)])
                    }
                    "source" | "." | "Include" => {
                        self.one_of(&[join(dir, &source), join("", &source)])
                    }
                    _ => Ok(Reach::Unknown),
                }
            }
            "powershell" => self.powershell_reach(dir, row, &source),
            "r" => self.r_reach(importer, row, &source, runtime_state),
            "ruby" => self.ruby_reach(importer, row, &source, runtime_state),
            "fsharp" => Ok(match row.form.as_str() {
                "#load" if !runtime_state => self.one_of(&[join(dir, &source)])?,
                _ => Reach::Unknown,
            }),
            "dart" => self.dart_reach(dir, &source),
            "php" => {
                if row.relative_to == "file" {
                    self.one_of(&[join(dir, &source)])
                } else if runtime_state || source.starts_with('/') || source.contains("://") {
                    Ok(Reach::Unknown)
                } else {
                    self.one_of(&[join(dir, &source), join("", &source)])
                }
            }
            "gdscript" => {
                if let Some(rest) = source.strip_prefix("res://") {
                    match self.godot_root(importer) {
                        Some(root) => self.one_of(&[join(&root, rest)]),
                        None => Ok(Reach::Unknown),
                    }
                } else if source.contains("://") {
                    Ok(Reach::Unknown)
                } else {
                    self.one_of(&[join(dir, &source)])
                }
            }
            _ => Ok(Reach::Unknown),
        }
    }

    /// A C++ `using` or namespace alias names a namespace, not a file: `std` is the runtime
    /// library, and a namespace that no project file declares is outside the project.
    fn cpp_namespace_reach(&self, source: &str) -> Result<Reach, QueryError> {
        let head = source
            .trim_start_matches("::")
            .split("::")
            .next()
            .unwrap_or_default();
        if head == "std" {
            return Ok(Reach::Outside);
        }
        let declared = self.with_statement(CPP_NAMESPACE_SQL, |stmt| stmt.exists(params![head]))?;
        Ok(if declared {
            Reach::Unknown
        } else {
            Reach::Outside
        })
    }

    /// A PowerShell path from `$PSScriptRoot` names a file beside the script. A relative path
    /// resolves from the working directory, read as the script's folder or the project root. A
    /// module name goes through the module path, which the index cannot see.
    fn powershell_reach(
        &self,
        dir: &str,
        row: &LoadRow,
        source: &str,
    ) -> Result<Reach, QueryError> {
        if !matches!(
            row.form.as_str(),
            "." | "Import-Module" | "using module" | "RootModule" | "NestedModules"
        ) {
            return Ok(Reach::Unknown);
        }
        if let Some(rest) = source
            .strip_prefix("$PSScriptRoot")
            .or_else(|| source.strip_prefix("${PSScriptRoot}"))
        {
            if rest.contains('$') {
                return Ok(Reach::Unknown);
            }
            return self.one_of(&[join(dir, rest.trim_start_matches('/'))]);
        }
        let path_like = source.contains('/')
            || [".ps1", ".psm1", ".psd1"]
                .iter()
                .any(|extension| source.to_ascii_lowercase().ends_with(extension));
        if source.contains('$') || source.starts_with('/') || !path_like {
            return Ok(Reach::Unknown);
        }
        if matches!(row.form.as_str(), "RootModule" | "NestedModules") {
            return self.one_of(&[join(dir, source)]);
        }
        self.one_of(&[join(dir, source), join("", source)])
    }

    /// An R `source()` path resolves from the working directory, read as the project root or the
    /// script's folder. A package load is outside unless a `DESCRIPTION` of the project names the
    /// package, whose files the index cannot tie to one load.
    fn r_reach(
        &mut self,
        importer: &str,
        row: &LoadRow,
        source: &str,
        runtime_state: bool,
    ) -> Result<Reach, QueryError> {
        match row.form.as_str() {
            "source()" if !runtime_state && !source.starts_with('/') => {
                self.one_of(&[join("", source), join(parent_dir(importer), source)])
            }
            "library()"
            | "require()"
            | "requireNamespace()"
            | "pacman::p_load()"
            | "import()"
            | "importFrom()"
            | "importClassesFrom()"
            | "importMethodsFrom()" => Ok(match self.r_package_dir(importer, source) {
                Some(_) => Reach::Unknown,
                None => Reach::Outside,
            }),
            _ => Ok(Reach::Unknown),
        }
    }

    /// A Ruby `require_relative` names a file beside the importer. `require` searches the load
    /// path, read as each folder above the importer and its `lib`; a source that names no
    /// project file is a gem or the standard library. `load` reads from the working directory.
    fn ruby_reach(
        &mut self,
        importer: &str,
        row: &LoadRow,
        source: &str,
        runtime_state: bool,
    ) -> Result<Reach, QueryError> {
        if runtime_state || source.starts_with('/') {
            return Ok(Reach::Unknown);
        }
        let dir = parent_dir(importer);
        let with_rb = if has_extension(source) {
            source.to_string()
        } else {
            format!("{source}.rb")
        };
        match row.form.as_str() {
            "require_relative" => self.one_of(&[join(dir, &with_rb)]),
            "load" => self.one_of(&[join("", source), join(dir, source)]),
            "require" | "autoload" => {
                if source.starts_with('.') {
                    return self.one_of(&[join("", &with_rb), join(dir, &with_rb)]);
                }
                let mut candidates = Vec::new();
                let mut folder = Some(dir);
                while let Some(current) = folder {
                    candidates.push(join(current, &with_rb));
                    candidates.push(join(current, &format!("lib/{with_rb}")));
                    folder = (!current.is_empty()).then(|| parent_dir(current));
                }
                let found = self.indexed_of(&candidates)?;
                match found.as_slice() {
                    [only] => Ok(Reach::Files(vec![only.clone()])),
                    [] if self.files_by_suffix(&with_rb)?.is_empty() => Ok(Reach::Outside),
                    _ => Ok(Reach::Unknown),
                }
            }
            _ => Ok(Reach::Unknown),
        }
    }

    /// A Dart `dart:` library is the runtime; `package:name/x.dart` is `lib/x.dart` of the
    /// project package whose `pubspec.yaml` has that name, else a package outside the project;
    /// any other URI is relative to the importer.
    fn dart_reach(&mut self, dir: &str, source: &str) -> Result<Reach, QueryError> {
        if source.starts_with("dart:") {
            return Ok(Reach::Outside);
        }
        if let Some(rest) = source.strip_prefix("package:") {
            let Some((package, path)) = rest.split_once('/') else {
                return Ok(Reach::Unknown);
            };
            let folders: Vec<String> = self
                .pubspecs()?
                .iter()
                .filter(|(name, _)| name == package)
                .map(|(_, folder)| folder.clone())
                .collect();
            if folders.is_empty() {
                return Ok(Reach::Outside);
            }
            let candidates: Vec<Option<String>> = folders
                .iter()
                .map(|folder| join(folder, &format!("lib/{path}")))
                .collect();
            return self.one_of(&candidates);
        }
        if source.contains("://") || source.starts_with('/') || !source.ends_with(".dart") {
            return Ok(Reach::Unknown);
        }
        self.one_of(&[join(dir, source)])
    }

    /// The package name and folder of each indexed `pubspec.yaml`.
    fn pubspecs(&mut self) -> Result<Rc<Vec<(String, String)>>, QueryError> {
        if let Some(pubspecs) = &self.loads.pubspecs {
            return Ok(pubspecs.clone());
        }
        let paths = self.with_statement(PUBSPECS_SQL, |stmt| {
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let mut pubspecs = Vec::new();
        for path in paths {
            let Ok(text) = std::fs::read_to_string(self.root.join(&path)) else {
                continue;
            };
            let name = text.lines().find_map(|line| {
                let value = line.strip_prefix("name:")?.trim();
                let value = value.split('#').next().unwrap_or_default().trim();
                Some(value.trim_matches(['"', '\'']).to_string())
            });
            if let Some(name) = name.filter(|name| !name.is_empty()) {
                pubspecs.push((name, parent_dir(&path).to_string()));
            }
        }
        let pubspecs = Rc::new(pubspecs);
        self.loads.pubspecs = Some(pubspecs.clone());
        Ok(pubspecs)
    }

    /// The folder above `importer` that holds `project.godot`, where `res://` starts.
    fn godot_root(&self, importer: &str) -> Option<String> {
        let mut folder = parent_dir(importer);
        loop {
            if self.root.join(folder).join("project.godot").is_file() {
                return Some(folder.to_string());
            }
            if folder.is_empty() {
                return None;
            }
            folder = parent_dir(folder);
        }
    }

    /// The folder of the project R package named `package`: a `DESCRIPTION` with `Package:
    /// package` above `importer` or in a folder at the project root.
    pub(super) fn r_package_dir(&mut self, importer: &str, package: &str) -> Option<String> {
        let key = format!("{importer}\0{package}");
        if let Some(known) = self.loads.r_packages.get(&key) {
            return known.clone();
        }
        let names_package = |folder: &Path| {
            std::fs::read_to_string(folder.join("DESCRIPTION")).is_ok_and(|text| {
                text.lines().any(|line| {
                    line.strip_prefix("Package:")
                        .is_some_and(|name| name.trim() == package)
                })
            })
        };
        let mut found = None;
        let mut folder = parent_dir(importer);
        loop {
            if names_package(&self.root.join(folder)) {
                found = Some(folder.to_string());
                break;
            }
            if folder.is_empty() {
                break;
            }
            folder = parent_dir(folder);
        }
        if found.is_none()
            && let Ok(entries) = std::fs::read_dir(self.root)
        {
            found = entries
                .flatten()
                .filter(|entry| entry.path().is_dir())
                .find(|entry| names_package(&entry.path()))
                .map(|entry| entry.file_name().to_string_lossy().into_owned());
        }
        self.loads.r_packages.insert(key, found.clone());
        found
    }

    /// The files that `importer` loads, directly or through the files it loads, up to four
    /// steps and 64 files.
    fn load_scope(&mut self, importer: &str) -> Result<Rc<LoadScope>, QueryError> {
        if let Some(scope) = self.loads.scopes.get(importer) {
            return Ok(scope.clone());
        }
        let mut seen: HashSet<String> = HashSet::from([importer.to_string()]);
        let mut files: Vec<(String, usize)> = Vec::new();
        let mut complete = true;
        let mut frontier: Vec<(String, Option<usize>)> = vec![(importer.to_string(), None)];
        for _ in 0..MAX_LOAD_HOPS {
            let mut next = Vec::new();
            for (file, first_line) in frontier {
                let rows = self.load_directives(&file)?;
                for row in rows.iter().filter(|row| is_load(row)) {
                    let found = match self.load_reach(&file, row)? {
                        Reach::Files(found) if found.len() == 1 => found,
                        Reach::Outside => continue,
                        _ => {
                            complete = false;
                            continue;
                        }
                    };
                    for path in found {
                        if !seen.insert(path.clone()) {
                            continue;
                        }
                        if files.len() >= MAX_LOADED_FILES {
                            complete = false;
                            break;
                        }
                        let line = first_line.unwrap_or(row.line);
                        files.push((path.clone(), line));
                        next.push((path, Some(line)));
                    }
                }
            }
            if next.is_empty() {
                frontier = next;
                break;
            }
            frontier = next;
        }
        if !frontier.is_empty() {
            complete = false;
        }
        let scope = Rc::new(LoadScope { files, complete });
        self.loads
            .scopes
            .insert(importer.to_string(), scope.clone());
        Ok(scope)
    }

    fn defines_top_level(&mut self, path: &str, name: &str) -> Result<bool, QueryError> {
        let key = (path.to_string(), name.to_string());
        if let Some(known) = self.loads.defines.get(&key) {
            return Ok(*known);
        }
        let defines = !self
            .rows_at_path(DEFINES_TOP_LEVEL_SQL, path, &[&name], |r| {
                r.get::<_, i64>(0)
            })?
            .is_empty();
        self.loads.defines.insert(key, defines);
        Ok(defines)
    }

    /// Whether code outside the index may load `path` into a scope that also holds other files:
    /// a project file loads it (or loads a path the index cannot resolve that could be it), or a
    /// framework loads it with its siblings (an R package folder, testthat, Rake, Composer's
    /// `files`).
    fn loaded_elsewhere(&mut self, path: &str, language: &str) -> Result<bool, QueryError> {
        if let Some(known) = self.loads.loaded_elsewhere.get(path) {
            return Ok(*known);
        }
        let mut loaded = self.loaded_by_framework(path, language);
        if !loaded {
            let pattern = format!("%{}%", crate::queries::escape_like(stem(path)));
            let loaders = self.with_statement(LOADER_ROWS_SQL, |stmt| {
                stmt.query_map(params![language, pattern], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })?;
            loaded = loaders.len() > MAX_LOADER_ROWS;
            let loaders: HashSet<String> = loaders
                .into_iter()
                .map(|loader| loader.replace('\\', "/"))
                .filter(|loader| loader != path)
                .collect();
            for loader in loaders {
                if loaded {
                    break;
                }
                let rows = self.load_directives(&loader)?;
                for row in rows.iter().filter(|row| is_load(row)) {
                    if match self.load_reach(&loader, row)? {
                        Reach::Files(found) => found.iter().any(|file| file == path),
                        Reach::Outside => false,
                        Reach::Unknown => {
                            row.source.contains(stem(path)) || row.name.contains(stem(path))
                        }
                    } {
                        loaded = true;
                        break;
                    }
                }
            }
        }
        self.loads.loaded_elsewhere.insert(path.to_string(), loaded);
        Ok(loaded)
    }

    fn loaded_by_framework(&mut self, path: &str, language: &str) -> bool {
        let dir = parent_dir(path);
        match language {
            "r" => {
                path.contains("tests/testthat/")
                    || dir.rsplit('/').next() == Some("R")
                        && self
                            .root
                            .join(parent_dir(dir))
                            .join("DESCRIPTION")
                            .is_file()
            }
            "ruby" => path.ends_with(".rake") || stem(path) == "Rakefile",
            "php" => {
                let mut folder = dir;
                loop {
                    let composer = self.root.join(folder).join("composer.json");
                    if std::fs::read_to_string(composer)
                        .is_ok_and(|text| text.contains("\"files\""))
                    {
                        return true;
                    }
                    if folder.is_empty() {
                        return false;
                    }
                    folder = parent_dir(folder);
                }
            }
            _ => false,
        }
    }

    /// The loaded files that define `name` at top level and that a bare call of `row` can see,
    /// when the files that the caller's file loads decide the call: they define the name, every
    /// load on the way resolves, and nothing else loads the caller's file into a shared scope.
    /// A site outside any function sees only the files loaded above it.
    fn loaded_definers(
        &mut self,
        row: &NameRow,
        name: &str,
        group: &[&Symbol],
    ) -> Result<Option<Vec<String>>, QueryError> {
        if !loads_into_global_scope(&row.language) || !row.is_bare_use() || !row.is_call() {
            return Ok(None);
        }
        let scope = self.load_scope(&row.path)?;
        if !scope.complete || scope.files.is_empty() {
            return Ok(None);
        }
        if row.language == "php" && self.php_namespaced(&row.path)? {
            return Ok(None);
        }
        let mut definers = Vec::new();
        for (file, line) in &scope.files {
            if row.caller_id.is_none() && *line >= row.site_line {
                continue;
            }
            let member_file = group
                .iter()
                .any(|member| member.path.replace('\\', "/") == *file);
            if member_file || self.defines_top_level(file, name)? {
                definers.push(file.clone());
            }
        }
        if definers.is_empty() || self.loaded_elsewhere(&row.path, &row.language)? {
            return Ok(None);
        }
        if row.language == "ruby"
            && self.with_statement(OWNED_METHOD_SQL, |stmt| {
                stmt.exists(params![name, row.language])
            })?
        {
            return Ok(None);
        }
        Ok(Some(definers))
    }

    fn php_namespaced(&self, path: &str) -> Result<bool, QueryError> {
        Ok(!self
            .rows_at_path(PHP_NAMESPACE_SQL, path, &[], |r| r.get::<_, i64>(0))?
            .is_empty())
    }

    /// The verdict on a bare Bash, PowerShell, R, PHP, or Ruby call of a top-level function that
    /// the caller's file loads, or a C or C++ call that its includes decide.
    pub(super) fn loaded_name_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if matches!(row.language.as_str(), "c" | "cpp") {
            return self.include_verdict(row, spec);
        }
        let group: Vec<&Symbol> = spec.group.iter().collect();
        let eligible = group.iter().all(|member| {
            member.parent_symbol_id.is_none()
                && member.language == row.language
                && member.path.replace('\\', "/") != row.path
        });
        if !eligible {
            return Ok(None);
        }
        let Some(definers) = self.loaded_definers(row, &spec.name, &group)? else {
            return Ok(None);
        };
        let reached = group
            .iter()
            .any(|member| definers.contains(&member.path.replace('\\', "/")));
        Ok(match (reached, definers.len()) {
            (true, 1) => Some(Verdict::new(SiteLabel::Proven, IMPORT_BASIS, false)),
            (true, _) => None,
            (false, _) => Some(Verdict::new(
                SiteLabel::Excluded,
                format!(
                    "the file loads `{}` from {}",
                    spec.name,
                    display_list(&definers)
                ),
                false,
            )),
        })
    }

    /// Whether the other definition `other` cannot reach `row`: the files the caller's file
    /// loads decide a bare call and do not hold it, a C or C++ `static` function lives in a file
    /// that the caller's file does not include, a C or C++ definition is outside the files the
    /// site can see, or a Dart prefix or GDScript constant names other files.
    pub(super) fn load_excludes(
        &mut self,
        row: &NameRow,
        other: &Definition,
    ) -> Result<bool, QueryError> {
        let c_family = matches!(row.language.as_str(), "c" | "cpp")
            && matches!(other.language.as_str(), "c" | "cpp");
        if (other.language != row.language && !c_family) || other.path == row.path {
            return Ok(false);
        }
        if c_family {
            return Ok(self.static_out_of_reach(row, other)? || self.include_excludes(row, other)?);
        }
        if let Some(files) = self.receiver_files(row, &other.name)? {
            return Ok(files.excludes(other));
        }
        if let Some(scope) = self.dart_bare_scope(row, &other.name)? {
            return Ok(scope.excludes(other));
        }
        if !other.top_level {
            return Ok(false);
        }
        let Some(definers) = self.loaded_definers(row, &other.name, &[])? else {
            return Ok(false);
        };
        Ok(!definers.contains(&other.path))
    }

    /// A C or C++ `static` function in a file other than the caller's that the caller's file
    /// does not include: internal linkage keeps it out of every other translation unit.
    fn static_out_of_reach(
        &mut self,
        row: &NameRow,
        other: &Definition,
    ) -> Result<bool, QueryError> {
        if !row.is_bare_use()
            || !other.top_level
            || other.body_start_line.is_none()
            || !is_static_signature(&other.signature)
        {
            return Ok(false);
        }
        let scope = self.load_scope(&row.path)?;
        Ok(scope.complete && !scope.files.iter().any(|(file, _)| *file == other.path))
    }

    /// A bare C or C++ call. A declaration in a header that the caller's file includes is the
    /// one the call names; a definition in an included file is visible to it. A definition in a
    /// source file whose header the caller's file includes is likely, since linking, not the
    /// include, picks the definition. A `static` definition in a file it does not include is out
    /// of reach.
    fn include_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if !row.is_bare_use() || !row.is_call() {
            return Ok(None);
        }
        let arity_rejects = spec
            .arity
            .as_ref()
            .zip(row.arg_count)
            .is_some_and(|(arity, count)| {
                arity
                    .exclusion_basis(&spec.name, &row.language, count)
                    .is_some()
            });
        if arity_rejects {
            return Ok(None);
        }
        let members: Vec<&Symbol> = spec.group.iter().collect();
        if members.iter().any(|member| {
            member.parent_symbol_id.is_some()
                || !matches!(member.language.as_str(), "c" | "cpp")
                || member.path.replace('\\', "/") == row.path
        }) {
            return Ok(None);
        }
        let scope = self.load_scope(&row.path)?;
        let included = |path: &str| scope.files.iter().any(|(file, _)| file == path);
        let paths: Vec<String> = members
            .iter()
            .map(|member| member.path.replace('\\', "/"))
            .collect();
        if paths.iter().any(|path| included(path)) {
            return Ok(Some(Verdict::new(SiteLabel::Proven, IMPORT_BASIS, false)));
        }
        let all_static = members.iter().all(|member| {
            member.body_start_line.is_some()
                && is_static_signature(member.signature.as_deref().unwrap_or_default())
        });
        if all_static && scope.complete {
            return Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!("`{}` is static in `{}`", spec.name, paths[0]),
                false,
            )));
        }
        if all_static
            || members
                .iter()
                .any(|member| member.body_start_line.is_none())
        {
            return Ok(None);
        }
        for (file, _) in &scope.files {
            if paths.iter().any(|path| stem(path) == stem(file))
                && self.defines_top_level(file, &spec.name)?
            {
                return Ok(Some(Verdict::new(
                    SiteLabel::Likely,
                    format!("the included `{file}` declares it"),
                    false,
                )));
            }
        }
        Ok(None)
    }

    /// The verdict on a call through a Dart import prefix or a GDScript `preload` constant.
    pub(super) fn receiver_file_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        let Some(files) = self.receiver_files(row, &spec.name)? else {
            return Ok(None);
        };
        let members: Vec<Definition> = spec
            .group
            .iter()
            .filter_map(|member| self.definition_by_id(&member.symbol_id).transpose())
            .collect::<Result<_, _>>()?;
        if members.len() != spec.group.len() {
            return Ok(None);
        }
        if members.iter().any(|member| files.reaches(member)) {
            return Ok((files.complete || row.language == "gdscript")
                .then(|| Verdict::new(SiteLabel::Proven, IMPORT_BASIS, false)));
        }
        if !members.iter().all(|member| files.excludes(member)) {
            return Ok(None);
        }
        let verb = if row.language == "gdscript" {
            "loaded"
        } else {
            "imported"
        };
        Ok(Some(Verdict::new(
            SiteLabel::Excluded,
            format!(
                "receiver `{}` is {verb} from `{}`",
                files.receiver, files.source
            ),
            false,
        )))
    }

    /// The files a Dart import prefix or a GDScript `preload` constant names, with the files
    /// they export or extend, when the receiver of `row` is one.
    fn receiver_files(
        &mut self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<ReceiverFiles>, QueryError> {
        if !matches!(row.language.as_str(), "dart" | "gdscript")
            || !row.namespace.is_empty()
            || row.hidden_receiver
        {
            return Ok(None);
        }
        let Some(receiver) = row
            .receiver
            .as_deref()
            .filter(|r| is_plain_identifier(r) && !SELF_RECEIVERS.contains(r))
        else {
            return Ok(None);
        };
        if row.language == "dart" {
            self.dart_prefix_files(row, receiver, name)
        } else {
            self.gdscript_constant_files(row, receiver, name)
        }
    }

    fn dart_prefix_files(
        &mut self,
        row: &NameRow,
        prefix: &str,
        name: &str,
    ) -> Result<Option<ReceiverFiles>, QueryError> {
        if !self
            .rows_at_path(LOCAL_NAMED_SQL, &row.path, &[&prefix], |r| {
                r.get::<_, i64>(0)
            })?
            .is_empty()
        {
            return Ok(None);
        }
        let Some(library) = self.dart_library(&row.path)? else {
            return Ok(None);
        };
        let rows = self.load_directives(&library)?;
        let prefixed: Vec<LoadRow> = rows
            .iter()
            .filter(|r| r.kind == "import" && r.form == "import" && r.alias == prefix)
            .cloned()
            .collect();
        let Some(first) = prefixed.first() else {
            return Ok(None);
        };
        let mut files = ReceiverFiles {
            receiver: prefix.to_string(),
            source: first.source.clone(),
            files: Vec::new(),
            owners: Vec::new(),
            complete: true,
        };
        self.dart_import_closure(&library, &prefixed, name, &mut files)?;
        Ok(Some(files))
    }

    /// The library that the Dart file `path` belongs to: the file itself, or the library that
    /// its `part of` directive names.
    fn dart_library(&mut self, path: &str) -> Result<Option<String>, QueryError> {
        let rows = self.load_directives(path)?;
        let Some(part_of) = rows.iter().find(|r| r.form == "part of") else {
            return Ok(Some(path.to_string()));
        };
        Ok(match self.load_reach(path, part_of)? {
            Reach::Files(found) if found.len() == 1 => Some(found[0].clone()),
            _ => None,
        })
    }

    /// Adds to `files` the libraries that the Dart `imports` of `library` bring `name` from,
    /// with their parts and the files they export, up to four steps and 64 files.
    fn dart_import_closure(
        &mut self,
        library: &str,
        imports: &[LoadRow],
        name: &str,
        files: &mut ReceiverFiles,
    ) -> Result<(), QueryError> {
        let mut frontier = Vec::new();
        for import in imports.iter().filter(|import| import.admits(name)) {
            if import.conditional {
                files.complete = false;
            }
            match self.load_reach(library, import)? {
                Reach::Files(found) if found.len() == 1 => frontier.extend(found),
                Reach::Outside => {}
                _ => files.complete = false,
            }
        }
        let mut seen: HashSet<String> = files.files.iter().cloned().collect();
        for _ in 0..MAX_LOAD_HOPS {
            let mut next = Vec::new();
            for file in frontier {
                if !seen.insert(file.clone()) {
                    continue;
                }
                if files.files.len() >= MAX_LOADED_FILES {
                    files.complete = false;
                    break;
                }
                files.files.push(file.clone());
                let rows = self.load_directives(&file)?;
                for directive in rows.iter() {
                    let follows = directive.form == "part"
                        || directive.kind == "export" && directive.admits(name);
                    if !follows {
                        continue;
                    }
                    if directive.conditional {
                        files.complete = false;
                    }
                    match self.load_reach(&file, directive)? {
                        Reach::Files(found) if found.len() == 1 => next.extend(found),
                        Reach::Outside => {}
                        _ => files.complete = false,
                    }
                }
            }
            if next.is_empty() {
                frontier = next;
                break;
            }
            frontier = next;
        }
        if !frontier.is_empty() {
            files.complete = false;
        }
        Ok(())
    }

    /// The top-level definitions of `name` that a bare Dart call in the file of `row` can see: the
    /// files of its own library (its parts), which win, else the libraries that its imports
    /// without a prefix bring `name` from. `None` when the call is not a bare one, or when a class
    /// member of that name could take it through the implicit `this`.
    fn dart_bare_scope(
        &mut self,
        row: &NameRow,
        name: &str,
    ) -> Result<Option<DartScope>, QueryError> {
        if row.language != "dart" || !row.is_bare_use() || !row.is_call() {
            return Ok(None);
        }
        if self.with_statement(OWNED_METHOD_SQL, |stmt| stmt.exists(params![name, "dart"]))? {
            return Ok(None);
        }
        let Some(library) = self.dart_library(&row.path)? else {
            return Ok(None);
        };
        let rows = self.load_directives(&library)?;
        let mut own = vec![library.clone()];
        let mut complete = true;
        for part in rows.iter().filter(|r| r.form == "part") {
            match self.load_reach(&library, part)? {
                Reach::Files(found) if found.len() == 1 => own.extend(found),
                _ => complete = false,
            }
        }
        let mut definers = Vec::new();
        for file in &own {
            if self.defines_top_level(file, name)? {
                definers.push(file.clone());
            }
        }
        if !definers.is_empty() {
            return Ok(Some(DartScope {
                definers,
                own: true,
                complete,
            }));
        }
        let imports: Vec<LoadRow> = rows
            .iter()
            .filter(|r| r.kind == "import" && r.form == "import" && r.alias.is_empty())
            .cloned()
            .collect();
        let mut files = ReceiverFiles {
            receiver: String::new(),
            source: String::new(),
            files: Vec::new(),
            owners: Vec::new(),
            complete,
        };
        self.dart_import_closure(&library, &imports, name, &mut files)?;
        for file in &files.files {
            if self.defines_top_level(file, name)? {
                definers.push(file.clone());
            }
        }
        Ok(Some(DartScope {
            definers,
            own: false,
            complete: files.complete,
        }))
    }

    /// The verdict on a bare Dart call of a top-level function: a library sees only its own
    /// declarations and the ones its imports bring in.
    pub(super) fn dart_bare_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        if spec.group.iter().any(|member| {
            member.parent_symbol_id.is_some()
                || member.language != "dart"
                || member.path.replace('\\', "/") == row.path
        }) {
            return Ok(None);
        }
        let Some(scope) = self
            .dart_bare_scope(row, &spec.name)?
            .filter(|scope| scope.complete)
        else {
            return Ok(None);
        };
        let paths: Vec<String> = spec
            .group
            .iter()
            .map(|member| member.path.replace('\\', "/"))
            .collect();
        if paths.iter().any(|path| scope.definers.contains(path)) {
            return Ok((scope.definers.len() == 1).then(|| {
                let basis = if scope.own {
                    "same library"
                } else {
                    IMPORT_BASIS
                };
                Verdict::new(SiteLabel::Proven, basis, false)
            }));
        }
        Ok(Some(Verdict::new(
            SiteLabel::Excluded,
            scope.basis(&spec.name, &paths[0]),
            false,
        )))
    }

    /// A GDScript `const Helper = preload("res://helper.gd")` names the class of that file; a
    /// call through it reaches the nearest class in that class's chain of `extends` that
    /// defines the name.
    fn gdscript_constant_files(
        &mut self,
        row: &NameRow,
        constant: &str,
        name: &str,
    ) -> Result<Option<ReceiverFiles>, QueryError> {
        if let Some(caller) = row.caller_id.as_deref() {
            let locals = self.with_statement(
                "SELECT 1 FROM symbols WHERE parent_symbol_id = ?1 AND name = ?2 LIMIT 1",
                |stmt| stmt.exists(params![caller, constant]),
            )?;
            if locals {
                return Ok(None);
            }
        }
        let rows = self.load_directives(&row.path)?;
        let bound: Vec<LoadRow> = rows
            .iter()
            .filter(|r| r.name == constant && matches!(r.form.as_str(), "preload" | "load"))
            .cloned()
            .collect();
        let [import] = bound.as_slice() else {
            return Ok(None);
        };
        let Reach::Files(found) = self.load_reach(&row.path, import)? else {
            return Ok(None);
        };
        let [file] = found.as_slice() else {
            return Ok(None);
        };
        let mut files = ReceiverFiles {
            receiver: constant.to_string(),
            source: import.source.clone(),
            files: Vec::new(),
            owners: Vec::new(),
            complete: false,
        };
        let mut visited: HashSet<String> = HashSet::new();
        let mut current = file.clone();
        for _ in 0..MAX_BASE_HOPS {
            if !visited.insert(current.clone()) {
                break;
            }
            let classes = self.rows_at_path(FILE_CLASS_SQL, &current, &[], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let [(class, base)] = classes.as_slice() else {
                break;
            };
            let defines = !self
                .rows_at_path(FILE_CLASS_MEMBER_SQL, &current, &[&name], |r| {
                    r.get::<_, i64>(0)
                })?
                .is_empty();
            if defines {
                files.files.push(current);
                files.owners.push(class.clone());
                files.complete = true;
                break;
            }
            let base = base.trim().trim_matches('"').to_string();
            if base.is_empty() {
                files.complete = true;
                break;
            }
            if base.starts_with("res://") || base.ends_with(".gd") {
                let load = LoadRow {
                    source: base,
                    form: "extends".to_string(),
                    ..import.clone()
                };
                match self.load_reach(&current, &load)? {
                    Reach::Files(next) if next.len() == 1 => current = next[0].clone(),
                    _ => break,
                }
                continue;
            }
            let named = self.with_statement(NAMED_GDSCRIPT_CLASS_SQL, |stmt| {
                stmt.query_map(params![base], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()
            })?;
            match named.as_slice() {
                [] => {
                    files.complete = true;
                    break;
                }
                [next] => current = next.clone(),
                _ => break,
            }
        }
        Ok(Some(files))
    }

    /// Whether an import of the receiver `qualifier` of a row in `importer` reaches no project
    /// file: a source this module resolves, a C++ namespace, or an R package.
    pub(super) fn loaded_source_is_external(
        &mut self,
        importer: &str,
        language: &str,
        source: &str,
        form: &str,
        relative_to: &str,
    ) -> Result<bool, QueryError> {
        let row = LoadRow {
            line: 0,
            name: String::new(),
            kind: "import".to_string(),
            language: language.to_string(),
            source: source.to_string(),
            form: form.to_string(),
            relative_to: relative_to.to_string(),
            alias: String::new(),
            system: false,
            show: Vec::new(),
            hide: Vec::new(),
            conditional: false,
        };
        Ok(self.load_reach(importer, &row)? == Reach::Outside)
    }

    /// The verdict on an R call through a package namespace (`pkg::f()`): a package of the
    /// project reaches only its own `R` folder, and any other package is outside the project.
    pub(super) fn r_namespace_verdict(
        &mut self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        let [package] = row.namespace.as_slice() else {
            return Ok(None);
        };
        if row.language != "r" || !is_plain_identifier(package.replace('.', "_").as_str()) {
            return Ok(None);
        }
        Ok(Some(match self.r_package_dir(&row.path, package) {
            Some(folder) => {
                let r_folder = if folder.is_empty() {
                    "R/".to_string()
                } else {
                    format!("{folder}/R/")
                };
                if spec
                    .group
                    .iter()
                    .any(|member| member.path.replace('\\', "/").starts_with(&r_folder))
                {
                    Verdict::new(SiteLabel::Proven, "qualified path", false)
                } else {
                    Verdict::new(
                        SiteLabel::Excluded,
                        format!("package `{package}` holds only `{r_folder}`"),
                        false,
                    )
                }
            }
            None => Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{package}` is imported from `{package}`"),
                false,
            ),
        }))
    }
}

/// The files that hold the top-level definitions of a name that a bare Dart call can see.
struct DartScope {
    definers: Vec<String>,
    /// The definers are files of the caller's own library.
    own: bool,
    /// Every import and part on the way resolved.
    complete: bool,
}

impl DartScope {
    fn excludes(&self, definition: &Definition) -> bool {
        self.complete && definition.top_level && !self.definers.contains(&definition.path)
    }

    fn basis(&self, name: &str, path: &str) -> String {
        match (self.definers.first(), self.own) {
            (Some(definer), true) => format!("the library defines `{name}` in `{definer}`"),
            (Some(definer), false) => format!("the file imports `{name}` from `{definer}`"),
            (None, _) => format!("no import of the file reaches `{path}`"),
        }
    }
}

/// The files that a receiver bound by an import can reach.
struct ReceiverFiles {
    receiver: String,
    source: String,
    files: Vec<String>,
    /// For GDScript, the class of each file in `files`, which owns the members it reaches.
    owners: Vec<String>,
    /// Every file the receiver can reach is in `files`.
    complete: bool,
}

impl ReceiverFiles {
    fn reaches(&self, definition: &Definition) -> bool {
        if definition.language == "gdscript" {
            return self.files.iter().zip(&self.owners).any(|(file, owner)| {
                *file == definition.path && definition.owner_name.as_ref() == Some(owner)
            });
        }
        definition.top_level && self.files.contains(&definition.path)
    }

    /// Whether `definition` is out of reach of the receiver: every file it can reach is known,
    /// or a Dart prefix names only top-level declarations and `definition` is a member.
    fn excludes(&self, definition: &Definition) -> bool {
        !self.reaches(definition)
            && (self.complete || definition.language == "dart" && !definition.top_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_join_removes_dot_segments_and_refuses_to_climb_above_the_root() {
        assert_eq!(join("a/b", "../c/./d.sh").as_deref(), Some("a/c/d.sh"));
        assert_eq!(join("", "./lib/x.php").as_deref(), Some("lib/x.php"));
        assert_eq!(join("a", "../../x"), None);
    }

    #[test]
    fn a_static_signature_names_internal_linkage_before_its_parameters() {
        assert!(is_static_signature("static int add(int a)"));
        assert!(is_static_signature("static inline int sq(int a)"));
        assert!(!is_static_signature("int add(int a)"));
        assert!(!is_static_signature("int make(static_t a)"));
    }

    #[test]
    fn only_binding_forms_bind_a_receiver() {
        assert!(!binds_name("ruby", "require", false));
        assert!(binds_name("ruby", "autoload", false));
        assert!(!binds_name("php", "require_once", false));
        assert!(binds_name("php", "use", false));
        assert!(binds_name("cpp", "namespace", false));
        assert!(!binds_name("cpp", "#include", false));
        assert!(binds_name("r", "library()", false));
        assert!(!binds_name("r", "source()", false));
        assert!(!binds_name("java", "import", true));
        assert!(binds_name("gdscript", "preload", false));
    }
}
