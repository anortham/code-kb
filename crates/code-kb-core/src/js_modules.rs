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
    Accounting, IMPORT_BASIS, NameRow, QueryError, Spec, Verdict, is_test_label, language_family,
    shared_leading_segments,
};
use crate::models::SiteLabel;

const MAX_DEPTH: usize = 8;
const MAX_TARGETS: usize = 8;
const MAX_CONFIG_EXTENDS: usize = 4;
const SCRIPT_EXTENSIONS: [&str; 9] = ["ts", "tsx", "mts", "cts", "d.ts", "js", "jsx", "mjs", "cjs"];
const COMPILED_EXTENSIONS: [(&str, &[&str]); 4] = [
    (".js", &["ts", "tsx"]),
    (".jsx", &["tsx"]),
    (".mjs", &["mts"]),
    (".cjs", &["cts"]),
];
const UNRESOLVED_PREFIXES: [&str; 4] = ["#", "@/", "~/", "/"];

const FILE_EXISTS_SQL: &str = "SELECT 1 FROM files WHERE path = ?1 OR path = ?2 LIMIT 1";

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

/// What a written name reaches through the imports of its file.
#[derive(Clone)]
pub(super) enum Reach {
    /// The chain head is not an import of the file, or a local symbol hides it.
    Skip,
    Unknown,
    External {
        binding: String,
        source: String,
    },
    Definitions(Vec<Reached>),
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Target {
    Module(String),
    Definition(Reached),
    Unknown,
}

#[derive(Clone, PartialEq, Eq)]
enum Module {
    File(String),
    External,
    Unknown,
}

enum Bound {
    Skip,
    External(String),
    Targets(Vec<Target>),
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

struct Packages {
    list: Vec<Package>,
    /// False when a `package.json` could not be read as JSON, so a missing name proves nothing.
    complete: bool,
}

type ImportRows = Rc<[ImportRow]>;

/// The per-report memory of module lookups.
#[derive(Default)]
pub(super) struct JsCaches {
    files: RefCell<HashMap<String, Option<String>>>,
    modules: RefCell<HashMap<(String, String), Module>>,
    imports: RefCell<HashMap<(String, String), ImportRows>>,
    exports: RefCell<HashMap<String, Rc<Vec<ExportRow>>>>,
    definitions: RefCell<HashMap<(String, String), Vec<Reached>>>,
    locals: RefCell<HashMap<(String, String), bool>>,
    walks: RefCell<HashMap<(String, String), Vec<Target>>>,
    reaches: RefCell<HashMap<(String, String, String), Reach>>,
    packages: RefCell<Option<Rc<Packages>>>,
    remaps: RefCell<HashMap<String, bool>>,
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
/// each script extension, and the `index` file of a folder.
fn file_candidates(base: &str) -> Vec<String> {
    let mut candidates = vec![base.to_string()];
    for (compiled, sources) in COMPILED_EXTENSIONS {
        if let Some(stem) = base.strip_suffix(compiled) {
            candidates.extend(sources.iter().map(|ext| format!("{stem}.{ext}")));
        }
    }
    candidates.extend(SCRIPT_EXTENSIONS.iter().map(|ext| format!("{base}.{ext}")));
    candidates.extend(
        SCRIPT_EXTENSIONS
            .iter()
            .map(|ext| child(base, &format!("index.{ext}"))),
    );
    candidates
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

/// The relative `extends` targets of a `tsconfig.json` text, read without a JSON parser because
/// the file may hold comments.
fn extended_configs(text: &str) -> impl Iterator<Item = String> + '_ {
    let rest = text.split_once("\"extends\"").map_or("", |(_, rest)| {
        rest.trim_start().trim_start_matches(':').trim_start()
    });
    let listed = if rest.starts_with('[') {
        rest.split(']').next()
    } else {
        rest.split([',', '}', '\n']).next()
    };
    listed
        .unwrap_or("")
        .split('"')
        .skip(1)
        .step_by(2)
        .filter(|path| path.starts_with('.'))
        .map(|path| {
            if path.ends_with(".json") {
                path.to_string()
            } else {
                format!("{path}.json")
            }
        })
}

/// Whether the config at `path`, or a config it extends, maps import names to other places.
fn config_remaps(path: &Path, depth: usize) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.contains("\"paths\"")
        || text.contains("\"baseUrl\"")
        || depth < MAX_CONFIG_EXTENDS
            && extended_configs(&text)
                .any(|extended| config_remaps(&path.with_file_name(extended), depth + 1))
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
        match self.js_reach(&row.path, chain, &spec.name)? {
            Reach::Skip | Reach::Unknown => Ok(None),
            Reach::External { binding, source } => Ok(Some(Verdict::new(
                SiteLabel::Excluded,
                format!("receiver `{binding}` is imported from `{source}`"),
                false,
            ))),
            Reach::Definitions(reached) => self.js_definitions_verdict(row, spec, reached),
        }
    }

    fn js_definitions_verdict(
        &self,
        row: &NameRow,
        spec: &Spec,
        reached: Vec<Reached>,
    ) -> Result<Option<Verdict>, QueryError> {
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
        let target = &spec.group[0];
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

    /// What the written `chain` and `name` in `importer` reach, following imports and re-exports.
    fn js_reach(&self, importer: &str, chain: &[String], name: &str) -> Result<Reach, QueryError> {
        let key = (importer.to_string(), chain.join("."), name.to_string());
        cached(&self.js.reaches, key, || {
            let head = chain.first().map_or(name, String::as_str);
            let mut targets = match self.js_binding(importer, head)? {
                Bound::Skip => return Ok(Reach::Skip),
                Bound::External(source) => {
                    return Ok(Reach::External {
                        binding: head.to_string(),
                        source,
                    });
                }
                Bound::Targets(targets) => targets,
            };
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
                    _ => return Ok(Reach::Unknown),
                }
            }
            Ok(if found.is_empty() {
                Reach::Unknown
            } else {
                Reach::Definitions(found)
            })
        })
    }

    /// What the import that binds `head` in `importer` names.
    fn js_binding(&self, importer: &str, head: &str) -> Result<Bound, QueryError> {
        if self.js_has_local_symbol(importer, head)? {
            return Ok(Bound::Skip);
        }
        let imports = self.js_import_rows(importer, head)?;
        if imports.is_empty() {
            return Ok(Bound::Skip);
        }
        let mut targets = Vec::new();
        let mut external = None;
        for import in imports.iter() {
            if let Some(source) = &import.source
                && self.js_module(importer, source)? == Module::External
            {
                external = Some(source.clone());
                continue;
            }
            targets.extend(self.import_targets(importer, import, head, 0, &mut HashSet::new())?);
        }
        Ok(match external {
            Some(source) if targets.is_empty() => Bound::External(source),
            Some(_) => Bound::Targets(vec![Target::Unknown]),
            None => Bound::Targets(targets),
        })
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
        let Module::File(file) = self.js_module(importer, source)? else {
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
                    Module::File(target) if row.namespace => found.push(Target::Module(target)),
                    Module::File(target) => found.extend(self.walk_export(
                        &target,
                        local.as_deref().unwrap_or(name),
                        depth + 1,
                        seen,
                    )?),
                    _ => found.push(Target::Unknown),
                },
                (None, Some(local)) => found.extend(self.local_targets(file, local, depth, seen)?),
                (None, None) => found.push(Target::Unknown),
            }
        }
        if explicit.is_empty() && name != "default" {
            for row in rows.iter().filter(|row| row.star) {
                let module = match &row.source {
                    Some(source) => self.js_module(file, source)?,
                    None => Module::Unknown,
                };
                match module {
                    Module::File(target) => {
                        found.extend(self.walk_export(&target, name, depth + 1, seen)?)
                    }
                    _ => found.push(Target::Unknown),
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

    /// Whether `specifier` names a package outside the project, as far as the index can tell.
    pub(super) fn js_source_is_external(
        &self,
        importer: &str,
        specifier: &str,
    ) -> Result<bool, QueryError> {
        Ok(self.js_module(importer, specifier)? == Module::External)
    }

    pub(super) fn js_binds_import(&self, path: &str, name: &str) -> Result<bool, QueryError> {
        Ok(!self.js_import_rows(path, name)?.is_empty())
    }

    fn js_module(&self, importer: &str, specifier: &str) -> Result<Module, QueryError> {
        let key = (importer.to_string(), specifier.to_string());
        cached(&self.js.modules, key, || {
            if specifier.starts_with('.') {
                let base = normalize(&child(parent(importer), specifier));
                let file = match base {
                    Some(base) => self.js_file(&base)?,
                    None => None,
                };
                return Ok(file.map_or(Module::Unknown, Module::File));
            }
            self.package_module(importer, specifier)
        })
    }

    fn package_module(&self, importer: &str, specifier: &str) -> Result<Module, QueryError> {
        if specifier.starts_with("node:") {
            return Ok(Module::External);
        }
        if specifier.contains(':')
            || UNRESOLVED_PREFIXES
                .iter()
                .any(|prefix| specifier.starts_with(prefix))
        {
            return Ok(Module::Unknown);
        }
        let Some((name, subpath)) = split_package(specifier) else {
            return Ok(Module::Unknown);
        };
        let packages = self.js_packages()?;
        let mut named = packages.list.iter().filter(|package| package.name == name);
        match (named.next(), named.next()) {
            (Some(package), None) => self.package_entry(package, subpath),
            (None, _) if packages.complete && !self.js_remaps_imports(importer) => {
                Ok(Module::External)
            }
            _ => Ok(Module::Unknown),
        }
    }

    fn package_entry(&self, package: &Package, subpath: &str) -> Result<Module, QueryError> {
        let file = match &package.exports {
            Some(exports) => {
                let key = if subpath.is_empty() {
                    ".".to_string()
                } else {
                    format!("./{subpath}")
                };
                match exports.select(&key) {
                    Some((value, star)) => self.first_indexed(value, &package.dir, star)?,
                    None => None,
                }
            }
            None if subpath.is_empty() => {
                let mut found = None;
                for entry in package.entries.iter().map(String::as_str).chain(["index"]) {
                    if let Some(base) = normalize(&child(&package.dir, entry))
                        && let Some(file) = self.js_file(&base)?
                    {
                        found = Some(file);
                        break;
                    }
                }
                found
            }
            None => match normalize(&child(&package.dir, subpath)) {
                Some(base) => self.js_file(&base)?,
                None => None,
            },
        };
        Ok(file.map_or(Module::Unknown, Module::File))
    }

    /// The first target of `exports`, in file order, that is an indexed file.
    fn first_indexed(
        &self,
        exports: &Exports,
        dir: &str,
        star: Option<&str>,
    ) -> Result<Option<String>, QueryError> {
        let values: Vec<&Exports> = match exports {
            Exports::Target(target) => {
                let target = star.map_or_else(|| target.clone(), |star| target.replace('*', star));
                return match normalize(&child(dir, &target)) {
                    Some(base) => self.js_file(&base),
                    None => Ok(None),
                };
            }
            Exports::List(items) => items.iter().collect(),
            Exports::Conditions(entries) => entries.iter().map(|(_, value)| value).collect(),
            Exports::Other => return Ok(None),
        };
        for value in values {
            if let Some(file) = self.first_indexed(value, dir, star)? {
                return Ok(Some(file));
            }
        }
        Ok(None)
    }

    /// The `package.json` files of the project: the folders that hold an indexed file, and the
    /// folders above them, are read from disk because git-ignored manifests are not indexed.
    fn js_packages(&self) -> Result<Rc<Packages>, QueryError> {
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
        let mut packages = Packages {
            list: Vec::new(),
            complete: true,
        };
        for dir in directories {
            let Ok(bytes) = std::fs::read(self.root.join(&dir).join("package.json")) else {
                continue;
            };
            match serde_json::from_slice::<Manifest>(&bytes) {
                Ok(Manifest {
                    name: Some(name),
                    exports,
                    module,
                    main,
                    types,
                }) => packages.list.push(Package {
                    dir,
                    name,
                    exports,
                    entries: [module, main, types].into_iter().flatten().collect(),
                }),
                Ok(_) => {}
                Err(_) => packages.complete = false,
            }
        }
        let packages = Rc::new(packages);
        *self.js.packages.borrow_mut() = Some(packages.clone());
        Ok(packages)
    }

    /// Whether a `tsconfig.json` or `jsconfig.json` at or above the importer maps import names
    /// to other places with `paths` or `baseUrl`, directly or through `extends`.
    fn js_remaps_imports(&self, importer: &str) -> bool {
        let mut dir = parent(importer);
        loop {
            let remaps = *self
                .js
                .remaps
                .borrow_mut()
                .entry(dir.to_string())
                .or_insert_with(|| {
                    std::fs::read_dir(self.root.join(dir))
                        .into_iter()
                        .flatten()
                        .flatten()
                        .any(|entry| {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            (name.starts_with("tsconfig") || name.starts_with("jsconfig"))
                                && name.ends_with(".json")
                                && config_remaps(&entry.path(), 0)
                        })
                });
            if remaps {
                return true;
            }
            if dir.is_empty() {
                return false;
            }
            dir = parent(dir);
        }
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
            let rows = self.with_statement(IMPORT_ROWS_SQL, |stmt| {
                stmt.query_map(params![path, name], |row| {
                    Ok(ImportRow {
                        source: row.get(0)?,
                        imported: row.get(1)?,
                        namespace: row.get(2)?,
                        default: row.get(3)?,
                        commonjs: row.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })?;
            Ok(rows.into())
        })
    }

    fn js_export_rows(&self, path: &str) -> Result<Rc<Vec<ExportRow>>, QueryError> {
        cached(&self.js.exports, path.to_string(), || {
            let rows = self.with_statement(EXPORT_ROWS_SQL, |stmt| {
                stmt.query_map(params![path], |row| {
                    Ok(ExportRow {
                        exported: row.get(0)?,
                        local: row.get(1)?,
                        source: row.get(2)?,
                        star: row.get(3)?,
                        namespace: row.get(4)?,
                        default: row.get(5)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()
            })?;
            Ok(Rc::new(rows))
        })
    }

    fn js_definitions(&self, path: &str, name: &str) -> Result<Vec<Reached>, QueryError> {
        let key = (path.to_string(), name.to_string());
        cached(&self.js.definitions, key, || {
            let rows = self.with_statement(TOP_LEVEL_DEFINITIONS_SQL, |stmt| {
                stmt.query_map(params![path, name], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()
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
            self.with_statement(LOCAL_SYMBOL_SQL, |stmt| stmt.exists(params![path, name]))
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
            let before = row
                .site_column
                .zip(row.site_line.checked_sub(1).and_then(|i| lines.get(i)))
                .and_then(|(column, line)| line.get(..column));
            let Some(written) = before.map(written_qualifier) else {
                continue;
            };
            let typed = row.kind == "type_usage";
            row.chain = written.filter(|chain| typed || chain.is_empty());
        }
    }
}
