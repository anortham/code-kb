//! Follows the import and export rows of JavaScript and TypeScript files to the definitions a
//! written name reaches, so a site is proven or excluded by the module it names.

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::hash::Hash;
use std::path::Path;
use std::rc::Rc;

use rusqlite::params;
use serde::Deserialize;
use serde::de::{self, Deserializer, MapAccess, SeqAccess, Visitor};

use super::{
    Accounting, FILE_EXISTS_SQL, IMPORT_BASIS, NameRow, QueryError, Spec, Verdict, is_test_label,
    language_family, shared_leading_segments, split_line,
};
use crate::models::SiteLabel;

const MAX_DEPTH: usize = 8;
const MAX_TARGETS: usize = 8;
const SCRIPT_EXTENSIONS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];
const DECLARATION_EXTENSIONS: [&str; 3] = ["d.ts", "d.mts", "d.cts"];
const COMPILED_EXTENSIONS: [(&str, &[&str]); 4] = [
    (".js", &["ts", "tsx"]),
    (".jsx", &["tsx"]),
    (".mjs", &["mts"]),
    (".cjs", &["cts"]),
];
/// Sources that name an alias or a subpath import of a package, never a package outside it.
const ALIAS_PREFIXES: [&str; 4] = ["#", "@/", "~/", "/"];

const PROJECT_DIRECTORIES_SQL: &str = "SELECT DISTINCT rtrim(p, replace(p, '/', ''))
     FROM (SELECT replace(path, char(92), '/') AS p FROM files)";

const IMPORT_ROWS_SQL: &str = "SELECT json_extract(metadata_json, '$.source'),
            json_extract(metadata_json, '$.importedName'),
            COALESCE(json_extract(metadata_json, '$.isNamespace'), 0),
            COALESCE(json_extract(metadata_json, '$.isDefault'), 0),
            COALESCE(json_extract(metadata_json, '$.isCommonJS'), 0)
     FROM symbols
     WHERE kind = 'import' AND path = ?1 AND name = ?2 AND json_valid(metadata_json)";

const EXPORT_ROWS_SQL: &str = "SELECT json_extract(metadata_json, '$.exportedName'),
            json_extract(metadata_json, '$.localName'),
            json_extract(metadata_json, '$.source'),
            COALESCE(json_extract(metadata_json, '$.isStar'), 0),
            COALESCE(json_extract(metadata_json, '$.isNamespace'), 0),
            COALESCE(json_extract(metadata_json, '$.isDefault'), 0)
     FROM symbols
     WHERE kind = 'export' AND path = ?1 AND json_valid(metadata_json)";

const TOP_LEVEL_DEFINITIONS_SQL: &str =
    "SELECT symbol_id, kind, start_line, COALESCE(signature, '')
     FROM symbols
     WHERE path = ?1 AND +name = ?2 AND parent_symbol_id IS NULL
       AND kind NOT IN ('import', 'export')";

const LOCAL_SYMBOL_SQL: &str = "SELECT 1 FROM symbols
     WHERE path = ?1 AND +name = ?2 AND kind NOT IN ('import', 'export') LIMIT 1";

/// A top-level definition that a name reaches.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Reached {
    symbol_id: String,
    path: String,
    name: String,
    kind: String,
    line: usize,
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Target {
    Module(String),
    Definition(Reached),
    Unknown,
}

struct ImportRow {
    source: Option<String>,
    imported: Option<String>,
    namespace: bool,
    default: bool,
    commonjs: bool,
}

struct ExportRow {
    exported: Option<String>,
    local: Option<String>,
    source: Option<String>,
    star: bool,
    namespace: bool,
    default: bool,
}

impl ExportRow {
    fn exports(&self, name: &str) -> bool {
        if name == "default" {
            self.default
        } else {
            !self.default && !self.star && self.exported.as_deref() == Some(name)
        }
    }
}

/// A `package.json` `exports` value; condition order is the order of the file.
enum Exports {
    Target(String),
    List(Vec<Exports>),
    Conditions(Vec<(String, Exports)>),
    Other,
}

impl<'de> Deserialize<'de> for Exports {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ExportsVisitor;
        impl<'de> Visitor<'de> for ExportsVisitor {
            type Value = Exports;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a package exports value")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Exports, E> {
                Ok(Exports::Target(value.to_string()))
            }

            fn visit_unit<E: de::Error>(self) -> Result<Exports, E> {
                Ok(Exports::Other)
            }

            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Exports, A::Error> {
                let mut items = Vec::new();
                while let Some(item) = seq.next_element()? {
                    items.push(item);
                }
                Ok(Exports::List(items))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Exports, A::Error> {
                let mut entries = Vec::new();
                while let Some(entry) = map.next_entry()? {
                    entries.push(entry);
                }
                Ok(Exports::Conditions(entries))
            }
        }
        deserializer.deserialize_any(ExportsVisitor)
    }
}

impl Exports {
    /// The value for the subpath `key` (`.`, `./v4`) and the text a `*` pattern matched.
    fn select<'a>(&'a self, key: &'a str) -> Option<(&'a Exports, Option<&'a str>)> {
        match self {
            Exports::Conditions(entries) if entries.iter().any(|(k, _)| k.starts_with('.')) => {
                if let Some((_, value)) = entries.iter().find(|(k, _)| k == key) {
                    return Some((value, None));
                }
                entries
                    .iter()
                    .filter_map(|(pattern, value)| {
                        let (prefix, suffix) = pattern.split_once('*')?;
                        let star = key.strip_prefix(prefix)?.strip_suffix(suffix)?;
                        Some((prefix.len(), value, star))
                    })
                    .max_by_key(|(prefix_len, ..)| *prefix_len)
                    .map(|(_, value, star)| (value, Some(star)))
            }
            _ if key == "." => Some((self, None)),
            _ => None,
        }
    }
}

#[derive(Deserialize)]
struct Manifest {
    name: Option<String>,
    exports: Option<Exports>,
    module: Option<String>,
    main: Option<String>,
    types: Option<String>,
}

struct Package {
    dir: String,
    name: String,
    exports: Option<Exports>,
    entries: Vec<String>,
}

type ImportRows = Rc<[ImportRow]>;

/// The per-report memory of module lookups.
#[derive(Default)]
pub(super) struct JsCaches {
    files: RefCell<HashMap<String, Option<String>>>,
    modules: RefCell<HashMap<(String, String), Option<String>>>,
    imports: RefCell<HashMap<(String, String), ImportRows>>,
    exports: RefCell<HashMap<String, Rc<Vec<ExportRow>>>>,
    definitions: RefCell<HashMap<(String, String), Vec<Reached>>>,
    locals: RefCell<HashMap<(String, String), bool>>,
    walks: RefCell<HashMap<(String, String), Vec<Target>>>,
    reaches: RefCell<HashMap<(String, String, String), Vec<Reached>>>,
    packages: RefCell<Option<Rc<Vec<Package>>>>,
}

fn cached<K: Eq + Hash, V: Clone>(
    cache: &RefCell<HashMap<K, V>>,
    key: K,
    compute: impl FnOnce() -> Result<V, QueryError>,
) -> Result<V, QueryError> {
    if let Some(value) = cache.borrow().get(&key) {
        return Ok(value.clone());
    }
    let value = compute()?;
    cache.borrow_mut().insert(key, value.clone());
    Ok(value)
}

fn child(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// `path` without `.` and `..` segments, or `None` when it climbs above the project root.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The files a module path can name, best first: the path, its TypeScript twin, the path with
/// each script extension, and the `index` file of a folder, then the same with declaration
/// extensions, which hold no code.
fn file_candidates(base: &str) -> Vec<String> {
    let mut candidates = vec![base.to_string()];
    for (compiled, sources) in COMPILED_EXTENSIONS {
        if let Some(stem) = base.strip_suffix(compiled) {
            candidates.extend(sources.iter().map(|ext| format!("{stem}.{ext}")));
        }
    }
    for extensions in [&SCRIPT_EXTENSIONS[..], &DECLARATION_EXTENSIONS[..]] {
        candidates.extend(extensions.iter().map(|ext| format!("{base}.{ext}")));
        candidates.extend(
            extensions
                .iter()
                .map(|ext| child(base, &format!("index.{ext}"))),
        );
    }
    candidates
}

fn is_declaration(path: &str) -> bool {
    DECLARATION_EXTENSIONS.iter().any(|ext| {
        path.strip_suffix(ext)
            .is_some_and(|stem| stem.ends_with('.'))
    })
}

/// The first of `files`, in order, that is not a declaration file, else the first file.
fn first_script(files: Vec<String>) -> Option<String> {
    let script = files.iter().find(|file| !is_declaration(file));
    script.or(files.first()).cloned()
}

/// The package name and the subpath of a bare specifier (`@scope/name/sub` is `@scope/name`, `sub`).
fn split_package(specifier: &str) -> Option<(&str, &str)> {
    let mut slashes = specifier.match_indices('/').map(|(index, _)| index);
    let end = if specifier.starts_with('@') {
        slashes.next()?;
        slashes.next()
    } else {
        slashes.next()
    };
    let end = end.unwrap_or(specifier.len());
    Some((&specifier[..end], specifier[end..].trim_start_matches('/')))
}

/// The names written before `before`'s end as `a.b.` pairs, `None` when a `.` follows something
/// other than a name (`f().`, `x[0].`).
fn written_qualifier(before: &str) -> Option<Vec<String>> {
    let is_name = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    let mut rest = before.trim_end();
    let mut chain = Vec::new();
    while let Some(head) = rest.strip_suffix('.') {
        let head = head.trim_end();
        let name = head.trim_end_matches(is_name);
        let written = &head[name.len()..];
        if written.is_empty() {
            return None;
        }
        chain.push(written.to_string());
        rest = name.trim_end();
    }
    chain.reverse();
    Some(chain)
}

/// Keeps the definitions that carry the meaning of the use: a call reaches values, a type
/// usage reaches types when a type and a value share a name.
fn with_meaning(row: &NameRow, reached: Vec<Reached>) -> Vec<Reached> {
    if row.is_call() {
        return reached
            .into_iter()
            .filter(|found| !matches!(found.kind.as_str(), "type" | "interface"))
            .collect();
    }
    if row.kind != "type_usage" {
        return reached;
    }
    let typed: HashSet<(String, String)> = reached
        .iter()
        .filter(|found| matches!(found.kind.as_str(), "type" | "interface" | "class" | "enum"))
        .map(|found| (found.path.clone(), found.name.clone()))
        .collect();
    reached
        .into_iter()
        .filter(|found| {
            !(matches!(
                found.kind.as_str(),
                "function" | "method" | "variable" | "constant"
            ) && typed.contains(&(found.path.clone(), found.name.clone())))
        })
        .collect()
}

impl Accounting<'_> {
    /// The verdict the imports of the row's file give, or `None` when they do not decide it.
    pub(super) fn js_verdict(
        &self,
        row: &NameRow,
        spec: &Spec,
    ) -> Result<Option<Verdict>, QueryError> {
        let Some(chain) = row
            .chain
            .as_ref()
            .filter(|_| !row.hidden_receiver && language_family(&row.language) == "js")
        else {
            return Ok(None);
        };
        let reached = self.js_reach(&row.path, chain, &spec.name)?;
        self.js_definitions_verdict(row, spec, reached)
    }

    fn js_definitions_verdict(
        &self,
        row: &NameRow,
        spec: &Spec,
        reached: Vec<Reached>,
    ) -> Result<Option<Verdict>, QueryError> {
        let target = &spec.group[0];
        if !is_declaration(&target.path) && reached.iter().all(|found| is_declaration(&found.path))
        {
            return Ok(None);
        }
        let reached = with_meaning(row, reached);
        let in_group = |found: &Reached| {
            spec.group.iter().any(|member| {
                member.parent_symbol_id.is_none()
                    && member.name == found.name
                    && member.path.replace('\\', "/") == found.path
            })
        };
        let distinct = |keep: &dyn Fn(&Reached) -> bool| {
            reached
                .iter()
                .filter(|found| keep(found))
                .map(|found| (found.path.as_str(), found.name.as_str()))
                .collect::<BTreeSet<_>>()
                .len()
        };
        let (total, inside) = (distinct(&|_| true), distinct(&in_group));
        if total == 0 {
            return Ok(None);
        }
        if inside == total {
            let basis = if spec.builds_class {
                "builds the class"
            } else {
                IMPORT_BASIS
            };
            return Ok(Some(Verdict::new(SiteLabel::Proven, basis, true)));
        }
        if inside > 0 {
            return Ok(Some(Verdict::new(
                SiteLabel::Unresolved,
                format!("the import reaches {total} definitions"),
                true,
            )));
        }
        let nearest = reached.iter().min_by_key(|found| {
            (
                std::cmp::Reverse(shared_leading_segments(&found.path, &target.path)),
                &found.path,
                found.line,
            )
        });
        let Some(nearest) = nearest else {
            return Ok(None);
        };
        Ok(self
            .definition_by_id(&nearest.symbol_id)?
            .map(|other| Verdict::new(SiteLabel::Excluded, other.resolves_to(), false)))
    }

    /// The definitions the written `chain` and `name` in `importer` reach, following imports and
    /// re-exports; empty when the imports do not say.
    fn js_reach(
        &self,
        importer: &str,
        chain: &[String],
        name: &str,
    ) -> Result<Vec<Reached>, QueryError> {
        let key = (importer.to_string(), chain.join("."), name.to_string());
        cached(&self.js.reaches, key, || {
            let head = chain.first().map_or(name, String::as_str);
            let mut targets = self.js_binding(importer, head)?;
            let members = chain
                .iter()
                .skip(1)
                .map(String::as_str)
                .chain((!chain.is_empty()).then_some(name));
            for member in members {
                let mut next: Vec<Target> = Vec::new();
                for target in &targets {
                    match target {
                        Target::Module(file) => next.extend(self.js_export(file, member)?),
                        _ => next.push(Target::Unknown),
                    }
                }
                let mut seen = HashSet::new();
                next.retain(|target| seen.insert(target.clone()));
                targets = if next.len() > MAX_TARGETS {
                    vec![Target::Unknown]
                } else {
                    next
                };
            }
            let mut found = Vec::new();
            for target in targets {
                match target {
                    Target::Definition(definition) => found.push(definition),
                    _ => return Ok(Vec::new()),
                }
            }
            Ok(found)
        })
    }

    /// What the import that binds `head` in `importer` names; nothing when `head` is not an
    /// import of the file or a local symbol hides it.
    fn js_binding(&self, importer: &str, head: &str) -> Result<Vec<Target>, QueryError> {
        if self.js_has_local_symbol(importer, head)? {
            return Ok(Vec::new());
        }
        let mut targets = Vec::new();
        for import in self.js_import_rows(importer, head)?.iter() {
            targets.extend(self.import_targets(importer, import, head, 0, &mut HashSet::new())?);
        }
        Ok(targets)
    }

    fn import_targets(
        &self,
        importer: &str,
        import: &ImportRow,
        binding: &str,
        depth: usize,
        seen: &mut HashSet<(String, String)>,
    ) -> Result<Vec<Target>, QueryError> {
        let Some(source) = &import.source else {
            return Ok(vec![Target::Unknown]);
        };
        let Some(file) = self.js_module(importer, source)? else {
            return Ok(vec![Target::Unknown]);
        };
        if import.namespace {
            return Ok(vec![Target::Module(file)]);
        }
        let name = if import.default {
            "default"
        } else if let Some(imported) = &import.imported {
            imported
        } else if import.commonjs {
            return Ok(vec![Target::Unknown]);
        } else {
            binding
        };
        self.walk_export(&file, name, depth + 1, seen)
    }

    fn js_export(&self, file: &str, name: &str) -> Result<Vec<Target>, QueryError> {
        let key = (file.to_string(), name.to_string());
        cached(&self.js.walks, key, || {
            self.walk_export(file, name, 0, &mut HashSet::new())
        })
    }

    /// What `name` is when read from `file`: an explicit export row wins, and only without one
    /// do the `export *` sources count.
    fn walk_export(
        &self,
        file: &str,
        name: &str,
        depth: usize,
        seen: &mut HashSet<(String, String)>,
    ) -> Result<Vec<Target>, QueryError> {
        if depth > MAX_DEPTH {
            return Ok(vec![Target::Unknown]);
        }
        if !seen.insert((file.to_string(), name.to_string())) {
            return Ok(Vec::new());
        }
        let rows = self.js_export_rows(file)?;
        let explicit: Vec<&ExportRow> = rows.iter().filter(|row| row.exports(name)).collect();
        let mut found = Vec::new();
        for row in &explicit {
            match (&row.source, &row.local) {
                (Some(source), local) => match self.js_module(file, source)? {
                    Some(target) if row.namespace => found.push(Target::Module(target)),
                    Some(target) => found.extend(self.walk_export(
                        &target,
                        local.as_deref().unwrap_or(name),
                        depth + 1,
                        seen,
                    )?),
                    None => found.push(Target::Unknown),
                },
                (None, Some(local)) => found.extend(self.local_targets(file, local, depth, seen)?),
                (None, None) => found.push(Target::Unknown),
            }
        }
        if explicit.is_empty() && name != "default" {
            for row in rows.iter().filter(|row| row.star) {
                let module = match &row.source {
                    Some(source) => self.js_module(file, source)?,
                    None => None,
                };
                match module {
                    Some(target) => {
                        found.extend(self.walk_export(&target, name, depth + 1, seen)?)
                    }
                    None => found.push(Target::Unknown),
                }
            }
        }
        Ok(found)
    }

    /// What a local name in `file` is: its top-level definitions and the imports that bind it.
    fn local_targets(
        &self,
        file: &str,
        local: &str,
        depth: usize,
        seen: &mut HashSet<(String, String)>,
    ) -> Result<Vec<Target>, QueryError> {
        let mut found: Vec<Target> = self
            .js_definitions(file, local)?
            .into_iter()
            .map(Target::Definition)
            .collect();
        for import in self.js_import_rows(file, local)?.iter() {
            found.extend(self.import_targets(file, import, local, depth + 1, seen)?);
        }
        if found.is_empty() {
            found.push(Target::Unknown);
        }
        Ok(found)
    }

    /// Whether `specifier` names a package outside the project: an alias or a workspace package
    /// is not outside, and any other name takes the rule that does not read the packages.
    pub(super) fn js_source_is_external(&mut self, specifier: &str) -> Result<bool, QueryError> {
        let alias = ALIAS_PREFIXES
            .iter()
            .any(|prefix| specifier.starts_with(prefix));
        Ok(!alias
            && !self.js_is_workspace_package(specifier)?
            && self.source_is_external(specifier)?)
    }

    pub(super) fn js_binds_import(&self, path: &str, name: &str) -> Result<bool, QueryError> {
        Ok(!self.js_import_rows(path, name)?.is_empty())
    }

    fn js_is_workspace_package(&self, specifier: &str) -> Result<bool, QueryError> {
        let Some((name, _)) = split_package(specifier) else {
            return Ok(false);
        };
        Ok(self
            .js_packages()?
            .iter()
            .any(|package| package.name == name))
    }

    /// The indexed file the module `specifier` names from `importer`, when the index says.
    fn js_module(&self, importer: &str, specifier: &str) -> Result<Option<String>, QueryError> {
        let key = (importer.to_string(), specifier.to_string());
        cached(&self.js.modules, key, || {
            if specifier.starts_with('.') {
                return match normalize(&child(parent(importer), specifier)) {
                    Some(base) => self.js_file(&base),
                    None => Ok(None),
                };
            }
            let Some((name, subpath)) = split_package(specifier) else {
                return Ok(None);
            };
            let packages = self.js_packages()?;
            let mut named = packages.iter().filter(|package| package.name == name);
            match (named.next(), named.next()) {
                (Some(package), None) => self.package_entry(package, subpath),
                _ => Ok(None),
            }
        })
    }

    fn package_entry(
        &self,
        package: &Package,
        subpath: &str,
    ) -> Result<Option<String>, QueryError> {
        match &package.exports {
            Some(exports) => {
                let key = if subpath.is_empty() {
                    ".".to_string()
                } else {
                    format!("./{subpath}")
                };
                match exports.select(&key) {
                    Some((value, star)) => self.first_indexed(value, &package.dir, star),
                    None => Ok(None),
                }
            }
            None if subpath.is_empty() => {
                let mut files = Vec::new();
                for entry in package.entries.iter().map(String::as_str).chain(["index"]) {
                    if let Some(base) = normalize(&child(&package.dir, entry)) {
                        files.extend(self.js_file(&base)?);
                    }
                }
                Ok(first_script(files))
            }
            None => match normalize(&child(&package.dir, subpath)) {
                Some(base) => self.js_file(&base),
                None => Ok(None),
            },
        }
    }

    /// The first target of `exports`, in file order, that is an indexed file; a declaration file
    /// only when no target is a script.
    fn first_indexed(
        &self,
        exports: &Exports,
        dir: &str,
        star: Option<&str>,
    ) -> Result<Option<String>, QueryError> {
        let mut files = Vec::new();
        self.indexed_targets(exports, dir, star, &mut files)?;
        Ok(first_script(files))
    }

    fn indexed_targets(
        &self,
        exports: &Exports,
        dir: &str,
        star: Option<&str>,
        files: &mut Vec<String>,
    ) -> Result<(), QueryError> {
        match exports {
            Exports::Target(target) => {
                let target = star.map_or_else(|| target.clone(), |star| target.replace('*', star));
                if let Some(base) = normalize(&child(dir, &target)) {
                    files.extend(self.js_file(&base)?);
                }
            }
            Exports::List(items) => {
                for item in items {
                    self.indexed_targets(item, dir, star, files)?;
                }
            }
            Exports::Conditions(entries) => {
                for (_, value) in entries {
                    self.indexed_targets(value, dir, star, files)?;
                }
            }
            Exports::Other => {}
        }
        Ok(())
    }

    /// The `package.json` files of the project: the folders that hold an indexed file, and the
    /// folders above them, are read from disk because git-ignored manifests are not indexed.
    fn js_packages(&self) -> Result<Rc<Vec<Package>>, QueryError> {
        if let Some(packages) = &*self.js.packages.borrow() {
            return Ok(packages.clone());
        }
        let indexed = self.with_statement(PROJECT_DIRECTORIES_SQL, |stmt| {
            stmt.query_map([], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()
        })?;
        let mut directories = BTreeSet::new();
        for directory in &indexed {
            let mut dir = directory.trim_end_matches('/');
            while directories.insert(dir.to_string()) && !dir.is_empty() {
                dir = parent(dir);
            }
        }
        let mut packages = Vec::new();
        for dir in directories {
            let Ok(bytes) = std::fs::read(self.root.join(&dir).join("package.json")) else {
                continue;
            };
            if let Ok(Manifest {
                name: Some(name),
                exports,
                module,
                main,
                types,
            }) = serde_json::from_slice(&bytes)
            {
                packages.push(Package {
                    dir,
                    name,
                    exports,
                    entries: [module, main, types].into_iter().flatten().collect(),
                });
            }
        }
        let packages = Rc::new(packages);
        *self.js.packages.borrow_mut() = Some(packages.clone());
        Ok(packages)
    }

    /// The first indexed file `base` can name.
    fn js_file(&self, base: &str) -> Result<Option<String>, QueryError> {
        cached(&self.js.files, base.to_string(), || {
            for candidate in file_candidates(base) {
                let windows = candidate.replace('/', "\\");
                if self.with_statement(FILE_EXISTS_SQL, |stmt| {
                    stmt.exists(params![candidate, windows])
                })? {
                    return Ok(Some(candidate));
                }
            }
            Ok(None)
        })
    }

    fn js_import_rows(&self, path: &str, name: &str) -> Result<ImportRows, QueryError> {
        let key = (path.to_string(), name.to_string());
        cached(&self.js.imports, key, || {
            let rows = self.rows_at_path(IMPORT_ROWS_SQL, path, &[&name], |row| {
                Ok(ImportRow {
                    source: row.get(0)?,
                    imported: row.get(1)?,
                    namespace: row.get(2)?,
                    default: row.get(3)?,
                    commonjs: row.get(4)?,
                })
            })?;
            Ok(rows.into())
        })
    }

    fn js_export_rows(&self, path: &str) -> Result<Rc<Vec<ExportRow>>, QueryError> {
        cached(&self.js.exports, path.to_string(), || {
            let rows = self.rows_at_path(EXPORT_ROWS_SQL, path, &[], |row| {
                Ok(ExportRow {
                    exported: row.get(0)?,
                    local: row.get(1)?,
                    source: row.get(2)?,
                    star: row.get(3)?,
                    namespace: row.get(4)?,
                    default: row.get(5)?,
                })
            })?;
            Ok(Rc::new(rows))
        })
    }

    fn js_definitions(&self, path: &str, name: &str) -> Result<Vec<Reached>, QueryError> {
        let key = (path.to_string(), name.to_string());
        cached(&self.js.definitions, key, || {
            let rows = self.rows_at_path(TOP_LEVEL_DEFINITIONS_SQL, path, &[&name], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            Ok(rows
                .into_iter()
                .filter(|(.., signature)| !is_test_label(signature))
                .map(|(symbol_id, kind, line, _)| Reached {
                    symbol_id,
                    path: path.to_string(),
                    name: name.to_string(),
                    kind,
                    line: line as usize,
                })
                .collect())
        })
    }

    fn js_has_local_symbol(&self, path: &str, name: &str) -> Result<bool, QueryError> {
        let key = (path.to_string(), name.to_string());
        cached(&self.js.locals, key, || {
            let rows =
                self.rows_at_path(LOCAL_SYMBOL_SQL, path, &[&name], |row| row.get::<_, i64>(0))?;
            Ok(!rows.is_empty())
        })
    }
}

/// Reads the names written before each JavaScript use whose qualifier julie did not record, so
/// a type usage (`z.core.output<T>`) gets its chain, and drops the chain of a use that follows a
/// `.` after something other than a name.
pub(super) fn complete_js_chains(root: &Path, rows: &mut [NameRow]) {
    let mut by_path: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, row) in rows.iter().enumerate() {
        let unqualified = row.chain.as_ref().is_some_and(Vec::is_empty);
        if language_family(&row.language) == "js"
            && !row.hidden_receiver
            && (unqualified || row.kind == "type_usage")
        {
            by_path.entry(row.path.clone()).or_default().push(index);
        }
    }
    for (path, indices) in by_path {
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        let lines: Vec<&str> = text.lines().collect();
        for index in indices {
            let row = &mut rows[index];
            let Some((before, _)) = row
                .site_column
                .and_then(|column| split_line(&lines, row.site_line, column))
            else {
                continue;
            };
            let written = written_qualifier(before);
            let typed = row.kind == "type_usage";
            row.chain = written.filter(|chain| typed || chain.is_empty());
        }
    }
}
