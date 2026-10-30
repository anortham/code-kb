//! What a Maven `pom.xml` says about the reactor that builds it: the `<modules>` it aggregates,
//! the modules that only a `<profile>` adds, and the file its `<parent>` names.

use super::gradle::{holds, joined};
use super::{CommandFinder, parent_dir, shell_word};

/// The parts of one `pom.xml` that decide which build runs its tests.
#[derive(Debug)]
pub(super) struct Pom {
    /// The module paths of `<modules>` (Maven 4 `<subprojects>`), relative to the folder of the
    /// file with `/`, and `None` when the reader cannot know them.
    modules: Option<Vec<String>>,
    /// The module paths that only a `<profile>` lists.
    profile_modules: Vec<String>,
    /// Whether a profile lists a module that the reader cannot follow.
    profile_unknown: bool,
    parent: Parent,
    /// The `groupId`, `artifactId`, and `version` of the project.
    coordinates: Coordinates,
    /// The `groupId`, `artifactId`, and `version` that `<parent>` names.
    parent_coordinates: Coordinates,
}

/// Maven coordinates as the POM writes them; an empty part is absent.
#[derive(Clone, Debug, Default, PartialEq)]
struct Coordinates {
    group: String,
    artifact: String,
    version: String,
}

#[derive(Debug, PartialEq)]
enum Parent {
    /// No `<parent>`.
    None,
    /// An empty `<relativePath/>`: Maven takes the parent from a repository.
    Repository,
    /// The path that `<relativePath>` names, `../pom.xml` when it is absent.
    Path(String),
    /// A `<relativePath>` the reader cannot follow.
    Unknown,
}

/// How a `pom.xml` takes a folder into its reactor.
#[derive(Debug, PartialEq)]
enum Listing {
    Module,
    /// Only a profile lists it, so only that profile builds it.
    Profile,
    No,
    Unknown,
}

/// How Maven runs the tests of one project: the folder it runs from, its program there, and,
/// for a project of a multi-module build, the project path that `-pl` selects.
pub(super) struct MavenRun {
    pub(super) dir: String,
    pub(super) tool: String,
    pub(super) project: Option<String>,
}

impl MavenRun {
    /// The command that runs every test of the project. A module of a reactor runs to the
    /// `package` phase, so that the modules `-am` builds give it their packaged jars, as Maven
    /// gives an installed module: a plugin bound to `package` can change a jar (gson adds its
    /// `module-info` there), and in the `test` phase the reactor hands out only the classes
    /// folder.
    pub(super) fn command(&self) -> String {
        match &self.project {
            Some(project) => format!("{} package -pl {} -am", self.tool, shell_word(project)),
            None => format!("{} test", self.tool),
        }
    }

    /// The command that runs the JUnit method `name` of the class `class`, which carries its
    /// package. In a reactor the filter also reaches the projects that `-am` builds, where it
    /// matches nothing, and surefire fails such a project unless `failIfNoSpecifiedTests` is
    /// false.
    pub(super) fn test_command(&self, class: &str, name: &str) -> String {
        let mut command = format!(
            "{} -Dtest={}#{}",
            self.command(),
            shell_word(class),
            shell_word(name)
        );
        if self.project.is_some() {
            command.push_str(" -Dsurefire.failIfNoSpecifiedTests=false");
        }
        command
    }
}

impl Pom {
    /// The facts of `source`; a file that is not well-formed XML with a `<project>` root knows
    /// no modules.
    pub(super) fn parse(source: &str) -> Self {
        Self::read(source).unwrap_or(Self {
            modules: None,
            profile_modules: Vec::new(),
            profile_unknown: true,
            parent: Parent::Unknown,
            coordinates: Coordinates::default(),
            parent_coordinates: Coordinates::default(),
        })
    }

    fn read(source: &str) -> Option<Self> {
        let mut modules = Some(Vec::new());
        let mut profile_modules = Vec::new();
        let mut profile_unknown = false;
        let mut parent = Parent::None;
        let mut coordinates = Coordinates::default();
        let mut parent_coordinates = Coordinates::default();
        let mut model_version = String::new();
        let mut packaging = String::new();
        let mut stack: Vec<String> = Vec::new();
        let mut text = String::new();
        let mut rest = source;
        let mut saw_root = false;
        while let Some(open) = rest.find('<') {
            text.push_str(&rest[..open]);
            rest = &rest[open..];
            if let Some(after) = rest.strip_prefix("<!--") {
                rest = &after[after.find("-->")? + 3..];
                continue;
            }
            if rest.starts_with("<?") {
                rest = &rest[rest.find("?>")? + 2..];
                continue;
            }
            if let Some(after) = rest.strip_prefix("<![CDATA[") {
                let end = after.find("]]>")?;
                text.push('<');
                text.push_str(&after[..end]);
                rest = &after[end + 3..];
                continue;
            }
            if rest.starts_with("<!") {
                rest = &rest[rest.find('>')? + 1..];
                continue;
            }
            let close = tag_end(rest)?;
            let tag = &rest[1..close];
            rest = &rest[close + 1..];
            if let Some(name) = tag.strip_prefix('/') {
                let name = local_name(name.trim());
                if stack.last().map(String::as_str) != Some(name) {
                    return None;
                }
                let path: Vec<&str> = stack.iter().map(String::as_str).collect();
                let value = text.trim();
                match path.as_slice() {
                    ["project", "modules", "module"] | ["project", "subprojects", "subproject"] => {
                        match module_path(value) {
                            Some(module) => {
                                if let Some(list) = modules.as_mut() {
                                    list.push(module);
                                }
                            }
                            None => modules = None,
                        }
                    }
                    ["project", "profiles", "profile", "modules", "module"]
                    | [
                        "project",
                        "profiles",
                        "profile",
                        "subprojects",
                        "subproject",
                    ] => match module_path(value) {
                        Some(module) => profile_modules.push(module),
                        None => profile_unknown = true,
                    },
                    ["project", "parent", "relativePath"] => parent = relative_path(value),
                    ["project", "groupId"] => coordinates.group = value.to_string(),
                    ["project", "artifactId"] => coordinates.artifact = value.to_string(),
                    ["project", "version"] => coordinates.version = value.to_string(),
                    ["project", "parent", "groupId"] => {
                        parent_coordinates.group = value.to_string()
                    }
                    ["project", "parent", "artifactId"] => {
                        parent_coordinates.artifact = value.to_string();
                    }
                    ["project", "parent", "version"] => {
                        parent_coordinates.version = value.to_string();
                    }
                    ["project", "modelVersion"] => model_version = value.to_string(),
                    ["project", "packaging"] => packaging = value.to_string(),
                    _ => {}
                }
                stack.pop();
                text.clear();
                continue;
            }
            let self_closing = tag.ends_with('/');
            let name = local_name(
                tag.trim_end_matches('/')
                    .split(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or_default(),
            );
            if name.is_empty() {
                return None;
            }
            if stack.is_empty() {
                if saw_root || name != "project" {
                    return None;
                }
                saw_root = true;
            }
            text.clear();
            let path_len = stack.len();
            if path_len == 1 && name == "parent" {
                parent = Parent::Path("../pom.xml".to_string());
            }
            if self_closing {
                if path_len == 2 && stack[1] == "parent" && name == "relativePath" {
                    parent = Parent::Repository;
                }
                continue;
            }
            stack.push(name.to_string());
        }
        if !stack.is_empty() || !saw_root {
            return None;
        }
        let discovers_subprojects = model_version == "4.1.0" && packaging == "pom";
        if discovers_subprojects && modules.as_ref().is_some_and(Vec::is_empty) {
            modules = None;
        }
        Some(Self {
            modules,
            profile_modules,
            profile_unknown,
            parent,
            coordinates,
            parent_coordinates,
        })
    }

    /// Whether this file is the parent that `wanted` names. Maven reads a local parent only
    /// when its `groupId` and `artifactId` match, and its version too when both name one;
    /// otherwise it takes the parent from a repository. `None` when a part that decides it
    /// interpolates a property or is absent.
    fn is_parent(&self, wanted: &Coordinates) -> Option<bool> {
        let inherited = |own: &str, parent: &str| -> String {
            if own.is_empty() { parent } else { own }.to_string()
        };
        let group = inherited(&self.coordinates.group, &self.parent_coordinates.group);
        let version = inherited(&self.coordinates.version, &self.parent_coordinates.version);
        let known = |part: &str| !part.is_empty() && !part.contains('$');
        if ![
            &group,
            &self.coordinates.artifact,
            &wanted.group,
            &wanted.artifact,
        ]
        .iter()
        .all(|part| known(part))
        {
            return None;
        }
        if group != wanted.group || self.coordinates.artifact != wanted.artifact {
            return Some(false);
        }
        if version.is_empty() || wanted.version.is_empty() || version == wanted.version {
            Some(true)
        } else if known(&version) && known(&wanted.version) {
            Some(false)
        } else {
            None
        }
    }

    /// Whether this file, in the folder `dir`, takes the folder `target` into its reactor. Both
    /// folders are relative to the project root.
    fn lists(&self, dir: &str, target: &str) -> Listing {
        let names = |modules: &[String]| {
            modules
                .iter()
                .any(|module| joined(dir, module).as_deref() == Some(target))
        };
        let Some(modules) = &self.modules else {
            return Listing::Unknown;
        };
        if names(modules) {
            Listing::Module
        } else if self.profile_unknown {
            Listing::Unknown
        } else if names(&self.profile_modules) {
            Listing::Profile
        } else {
            Listing::No
        }
    }

    /// Whether the file aggregates modules, or may.
    fn aggregates(&self) -> bool {
        self.modules
            .as_ref()
            .is_none_or(|modules| !modules.is_empty())
            || self.profile_unknown
            || !self.profile_modules.is_empty()
    }
}

/// The folder `to` as a path from the folder `from`, both relative to the project root.
fn path_from(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|part| !part.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|part| !part.is_empty()).collect();
    let shared = from
        .iter()
        .zip(&to)
        .take_while(|(left, right)| left == right)
        .count();
    let path: Vec<&str> = std::iter::repeat_n("..", from.len() - shared)
        .chain(to[shared..].iter().copied())
        .collect();
    if path.is_empty() {
        ".".to_string()
    } else {
        path.join("/")
    }
}

/// The index of the `>` that ends the tag at the start of `rest`, outside quoted attribute
/// values.
fn tag_end(rest: &str) -> Option<usize> {
    let mut quote = None;
    for (index, c) in rest.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(open), _) if c == open => quote = None,
            (None, '>') => return Some(index),
            _ => {}
        }
    }
    None
}

/// The element name without its namespace prefix.
fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// The folder a `<module>` names, relative to its `pom.xml`, or `None` when it interpolates a
/// property, holds an entity or a CDATA section (read as `<`), or names a POM file other than
/// `pom.xml`.
fn module_path(value: &str) -> Option<String> {
    if value.is_empty() || value.contains(['$', '&', '<']) {
        return None;
    }
    let path = value.replace('\\', "/");
    let path = path.trim_end_matches('/');
    let folder = match path.rsplit_once('/') {
        Some((folder, "pom.xml")) => folder,
        None if path == "pom.xml" => ".",
        _ if path.to_ascii_lowercase().ends_with(".xml") => return None,
        _ => path,
    };
    Some(folder.to_string())
}

fn relative_path(value: &str) -> Parent {
    if value.is_empty() {
        Parent::Repository
    } else if value.contains(['$', '&', '<']) {
        Parent::Unknown
    } else {
        Parent::Path(value.replace('\\', "/"))
    }
}

impl CommandFinder {
    /// How Maven runs the tests of the project whose `pom.xml` is in `build_dir`. A module of a
    /// multi-module build runs from its reactor root, the topmost folder whose `<modules>` chain
    /// reaches it, as `mvn package -pl module -am`, because run alone it cannot resolve the
    /// snapshots of the modules it depends on, and a plain run from the root runs every module.
    /// A project that no reactor lists keeps `mvn test` in its own folder, unless its parent is
    /// a local `pom.xml` that aggregates modules or is itself a module. A `pom.xml` that
    /// aggregates modules has `pom` packaging and builds no tests of its own, also when a
    /// reactor above lists it. `None` for those and when the reactor cannot be known with
    /// certainty: a module from a property or a profile, a POM file other than `pom.xml`,
    /// malformed XML, or two reactors that list the project.
    pub(super) fn maven_run(&mut self, build_dir: &str) -> Option<MavenRun> {
        if self.folder(build_dir).maven.as_ref()?.aggregates() {
            return None;
        }
        let mut root = build_dir.to_string();
        let mut steps = 0;
        while let Some(aggregator) = self.maven_aggregator(&root)? {
            steps += 1;
            if steps > 64 {
                return None;
            }
            root = aggregator;
        }
        let project = if root != build_dir {
            Some(path_from(&root, build_dir))
        } else if self.maven_parent_in_a_reactor(build_dir)? {
            return None;
        } else {
            None
        };
        Some(MavenRun {
            tool: self.maven_tool(&root),
            dir: root,
            project,
        })
    }

    /// The folder of the `pom.xml` whose reactor takes `dir` as a module: the topmost folder
    /// above it that lists it, or the folder of the file that its `<parent>` points at when that
    /// file lists it, whatever parent it is. `Some(None)` when none does, and `None` when the
    /// answer is uncertain.
    fn maven_aggregator(&mut self, dir: &str) -> Option<Option<String>> {
        let mut found = None;
        let mut above = dir;
        while !above.is_empty() {
            above = parent_dir(above);
            let Some(pom) = self.folder(above).maven.as_ref() else {
                continue;
            };
            match pom.lists(above, dir) {
                Listing::Module => found = Some(above.to_string()),
                Listing::No => {}
                Listing::Profile | Listing::Unknown => return None,
            }
        }
        if let Some(parent) = self.maven_parent_path(dir)?
            && !holds(&parent, dir)
        {
            match self.folder(&parent).maven.as_ref()?.lists(&parent, dir) {
                Listing::Module if found.is_none() => found = Some(parent),
                Listing::No => {}
                Listing::Module | Listing::Profile | Listing::Unknown => return None,
            }
        }
        Some(found)
    }

    /// The folder of the local `pom.xml` that the `<parent>` of the `pom.xml` in `dir` is.
    /// `Some(None)` when Maven takes the parent from a repository: the file that
    /// `maven_parent_path` names is none, or it has other coordinates than `<parent>`
    /// names. `None` when that cannot be known.
    fn maven_parent_dir(&mut self, dir: &str) -> Option<Option<String>> {
        let Some(parent) = self.maven_parent_path(dir)? else {
            return Some(None);
        };
        let wanted = self.folder(dir).maven.as_ref()?.parent_coordinates.clone();
        let is_parent = self.folder(&parent).maven.as_ref()?.is_parent(&wanted)?;
        Some(is_parent.then_some(parent))
    }

    /// The folder of the local `pom.xml` that the `<parent>` of the `pom.xml` in `dir` points
    /// at, whatever project it is. `Some(None)` when it points at none: no `<parent>`, an
    /// empty `<relativePath/>`, or a path with no file under the project root. `None` when the
    /// path cannot be followed or names a POM file other than `pom.xml`.
    fn maven_parent_path(&mut self, dir: &str) -> Option<Option<String>> {
        let path = match &self.folder(dir).maven.as_ref()?.parent {
            Parent::None | Parent::Repository => return Some(None),
            Parent::Unknown => return None,
            Parent::Path(path) => path.trim_end_matches('/').to_string(),
        };
        let file = if path.to_ascii_lowercase().ends_with(".xml") {
            path
        } else {
            format!("{path}/pom.xml")
        };
        let Some(file) = joined(dir, &file) else {
            return Some(None);
        };
        if !self.root.join(&file).is_file() {
            return Some(None);
        }
        let (folder, name) = file.rsplit_once('/').unwrap_or(("", file.as_str()));
        (name == "pom.xml").then(|| Some(folder.to_string()))
    }

    /// Whether the local parent of the `pom.xml` in `dir` belongs to a multi-module build, so
    /// that `dir`, which no reactor lists, may need modules that only that build makes.
    fn maven_parent_in_a_reactor(&mut self, dir: &str) -> Option<bool> {
        let Some(parent) = self.maven_parent_dir(dir)? else {
            return Some(false);
        };
        if self.folder(&parent).maven.as_ref()?.aggregates() {
            return Some(true);
        }
        Some(self.maven_aggregator(&parent)?.is_some())
    }

    /// The command that runs every test of the Maven build in `dir`: `mvn package` when its
    /// `pom.xml` aggregates modules, so that each module gets the jars of the modules it needs
    /// (`MavenRun::command`), else `mvn test`.
    pub(super) fn maven_build_command(&mut self, dir: &str) -> String {
        let phase = if self.folder(dir).maven.as_ref().is_none_or(Pom::aggregates) {
            "package"
        } else {
            "test"
        };
        format!("{} {phase}", self.maven_tool(dir))
    }

    /// The Maven program in `dir`: the wrapper there (`./mvnw`, or `mvnw.cmd` on Windows), else
    /// `mvn`.
    pub(super) fn maven_tool(&mut self, dir: &str) -> String {
        let folder = self.folder(dir);
        if cfg!(windows) && folder.mvnw_cmd {
            "mvnw.cmd".to_string()
        } else if folder.mvnw {
            "./mvnw".to_string()
        } else {
            "mvn".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pom(body: &str) -> Pom {
        Pom::parse(&format!(
            "<?xml version=\"1.0\"?>\n<!-- a <modules> comment -->\n<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n<modelVersion>4.0.0</modelVersion>\n{body}\n</project>\n"
        ))
    }

    #[test]
    fn a_pom_lists_its_modules_by_folder_and_by_pom_file() {
        let parsed = pom(
            "<modules>\n  <module>core</module>\n  <module>libs/extra/</module>\n  <module>tools/pom.xml</module>\n  <module>../sibling</module>\n</modules>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::Module);
        assert_eq!(parsed.lists("", "libs/extra"), Listing::Module);
        assert_eq!(parsed.lists("", "tools"), Listing::Module);
        assert_eq!(parsed.lists("parent", "sibling"), Listing::Module);
        assert_eq!(parsed.lists("", "libs"), Listing::No);
        assert!(parsed.aggregates());
    }

    #[test]
    fn a_module_that_only_a_profile_lists_is_no_reactor_module() {
        let parsed = pom(
            "<modules><module>core</module></modules>\n<profiles><profile><id>extra</id><modules><module>bench</module></modules></profile></profiles>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::Module);
        assert_eq!(parsed.lists("", "bench"), Listing::Profile);
        assert_eq!(parsed.lists("", "other"), Listing::No);
    }

    #[test]
    fn modules_the_reader_cannot_follow_make_every_listing_unknown() {
        for body in [
            "<modules><module>${core.dir}</module></modules>",
            "<modules><module>core/custom-pom.xml</module></modules>",
            "<modules><module><![CDATA[core]]></module></modules>",
            "<modules><module>core</module>",
            "<packaging>pom</packaging>",
        ] {
            let parsed = Pom::parse(&format!(
                "<project><modelVersion>{}</modelVersion>{body}</project>",
                if body.starts_with("<packaging>") {
                    "4.1.0"
                } else {
                    "4.0.0"
                }
            ));

            assert_eq!(parsed.lists("", "core"), Listing::Unknown, "{body}");
        }
    }

    #[test]
    fn a_profile_module_the_reader_cannot_follow_hides_only_unlisted_folders() {
        let parsed = pom(
            "<modules><module>core</module></modules><profiles><profile><modules><module>${x}</module></modules></profile></profiles>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::Module);
        assert_eq!(parsed.lists("", "other"), Listing::Unknown);
    }

    #[test]
    fn a_parent_defaults_to_the_pom_above_and_an_empty_relative_path_to_a_repository() {
        assert_eq!(pom("").parent, Parent::None);
        assert_eq!(
            pom("<parent><groupId>g</groupId></parent>").parent,
            Parent::Path("../pom.xml".to_string())
        );
        assert_eq!(
            pom("<parent><relativePath>../build/pom.xml</relativePath></parent>").parent,
            Parent::Path("../build/pom.xml".to_string())
        );
        assert_eq!(
            pom("<parent><relativePath/></parent>").parent,
            Parent::Repository
        );
        assert_eq!(
            pom("<parent><relativePath></relativePath></parent>").parent,
            Parent::Repository
        );
        assert_eq!(
            pom("<parent><relativePath>${p}</relativePath></parent>").parent,
            Parent::Unknown
        );
    }

    #[test]
    fn a_cdata_section_outside_the_modules_keeps_them_readable() {
        let parsed = pom(
            "<modules><module>core</module></modules>\n<build><plugins><plugin><configuration><bnd><![CDATA[\nExport-Package: <x>\n]]></bnd></configuration></plugin></plugins></build>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::Module);
        assert_eq!(parsed.lists("", "other"), Listing::No);
    }

    #[test]
    fn an_angle_bracket_in_a_quoted_attribute_value_does_not_end_its_tag() {
        let parsed = pom(
            "<modules><module>core</module></modules>\n<build><plugins><plugin><configuration><target><echo message=\"generated -> target\"/><echo message='a > b'></echo></target></configuration></plugin></plugins></build>",
        );
        let solo = pom(
            "<build><plugins><plugin><configuration><echo message=\"generated -> target\"/></configuration></plugin></plugins></build>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::Module);
        assert_eq!(parsed.lists("", "other"), Listing::No);
        assert_eq!(solo.modules, Some(Vec::new()));
        assert!(!solo.aggregates());
    }

    #[test]
    fn a_local_pom_is_the_parent_only_with_the_coordinates_that_parent_names() {
        let local = pom(
            "<parent><groupId>g</groupId><artifactId>top</artifactId><version>2</version></parent><artifactId>root</artifactId>",
        );
        let wanted = |group: &str, artifact: &str, version: &str| Coordinates {
            group: group.to_string(),
            artifact: artifact.to_string(),
            version: version.to_string(),
        };

        assert_eq!(local.is_parent(&wanted("g", "root", "2")), Some(true));
        assert_eq!(local.is_parent(&wanted("g", "root", "")), Some(true));
        assert_eq!(local.is_parent(&wanted("g", "other", "2")), Some(false));
        assert_eq!(local.is_parent(&wanted("h", "root", "2")), Some(false));
        assert_eq!(local.is_parent(&wanted("g", "root", "3")), Some(false));
        assert_eq!(local.is_parent(&wanted("g", "root", "${revision}")), None);
        assert_eq!(local.is_parent(&wanted("", "root", "2")), None);
        assert_eq!(pom("").is_parent(&wanted("g", "root", "2")), None);
    }

    #[test]
    fn a_dependency_or_plugin_element_named_module_is_no_reactor_module() {
        let parsed = pom(
            "<build><plugins><plugin><configuration><modules><module>core</module></modules></configuration></plugin></plugins></build>",
        );

        assert_eq!(parsed.lists("", "core"), Listing::No);
        assert!(!parsed.aggregates());
    }
}
