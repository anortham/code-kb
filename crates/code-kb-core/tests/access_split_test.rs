use code_kb_core::formatters::format_reference_report_page;
use code_kb_core::{
    AccessCounts, ReferenceReport, SiteAccess, SiteLabel, Workspace, find_julie_extract_binary,
    format_context_slices, format_reference_report, open_read_only,
    ops::{SymbolSelector, get_context_slice_selected_op},
    reference_report, safe_tempdir, scan_workspace, select_reference_target,
};
use std::fs;
use std::path::PathBuf;

struct Repo {
    root: PathBuf,
    db: PathBuf,
    _temp: tempfile::TempDir,
}

fn scanned_repo(files: &[(&str, &str)]) -> Repo {
    find_julie_extract_binary().expect("julie-extract binary must be present for tests");
    let temp = safe_tempdir();
    let root = temp.path().to_path_buf();
    for (path, content) in files {
        let full = root.join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, content).unwrap();
    }
    let db = root.join(".code-kb").join("artifact.db");
    scan_workspace(&Workspace::new(root.clone()), &db, true).expect("scan failed");
    Repo {
        root,
        db,
        _temp: temp,
    }
}

fn report(repo: &Repo, name: &str, path: &str) -> ReferenceReport {
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, name, Some(path)).unwrap();
    reference_report(&conn, &repo.root, &target, false).unwrap()
}

fn proven_rows(report: &ReferenceReport) -> Vec<(String, usize, Option<SiteAccess>)> {
    report
        .sites
        .iter()
        .filter(|site| site.label == SiteLabel::Proven)
        .map(|site| {
            (
                site.site.path.clone(),
                site.site.start_line.unwrap(),
                site.access,
            )
        })
        .collect()
}

const ONE_OF_EACH: AccessCounts = AccessCounts {
    writes: 1,
    read_writes: 2,
    reads: 1,
    unclassified: 0,
};

fn assert_writes_first(
    files: &[(&str, &str)],
    name: &str,
    defining_path: &str,
    expected: &[(&str, usize, SiteAccess)],
) {
    let repo = scanned_repo(files);
    let report = report(&repo, name, defining_path);
    let expected: Vec<(String, usize, Option<SiteAccess>)> = expected
        .iter()
        .map(|(path, line, access)| (path.to_string(), *line, Some(*access)))
        .collect();
    assert_eq!(proven_rows(&report), expected, "{report:#?}");
    assert_eq!(report.access_counts, Some(ONE_OF_EACH), "{report:#?}");
    let text = format_reference_report(&report, 20);
    assert!(
        text.contains(" · writes 1, read-writes 2, reads 1, unclassified 0\n"),
        "{text}"
    );
}

#[test]
fn python_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            (
                "model.py",
                "class Counter:\n    def __init__(self):\n        self.total = 0\n\n    def bump(self):\n        self.total += 1\n",
            ),
            (
                "use.py",
                "from model import Counter\n\n\ndef reset(c: Counter):\n    c.total = 0\n\n\ndef show(c: Counter):\n    return c.total\n\n\ndef step(c: Counter):\n    c.total = c.total + 1\n",
            ),
        ],
        "total",
        "model.py",
        &[
            ("use.py", 5, SiteAccess::Write),
            ("model.py", 6, SiteAccess::ReadWrite),
            ("use.py", 13, SiteAccess::ReadWrite),
            ("use.py", 9, SiteAccess::Read),
        ],
    );
}

#[test]
fn typescript_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            (
                "model.ts",
                "export class Counter {\n  total = 0;\n  bump() {\n    this.total += 1;\n  }\n}\n",
            ),
            (
                "use.ts",
                "import { Counter } from \"./model\";\n\nexport function reset(c: Counter) {\n  c.total = 0;\n}\n\nexport function show(c: Counter) {\n  return c.total;\n}\n\nexport function step(c: Counter) {\n  c.total = c.total + 1;\n}\n",
            ),
        ],
        "total",
        "model.ts",
        &[
            ("use.ts", 4, SiteAccess::Write),
            ("model.ts", 4, SiteAccess::ReadWrite),
            ("use.ts", 12, SiteAccess::ReadWrite),
            ("use.ts", 8, SiteAccess::Read),
        ],
    );
}

#[test]
fn java_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            (
                "Counter.java",
                "package app;\n\npublic class Counter {\n    public int total;\n\n    void bump() {\n        total += 1;\n    }\n}\n",
            ),
            (
                "Use.java",
                "package app;\n\npublic class Use {\n    void reset(Counter c) {\n        c.total = 0;\n    }\n\n    int show(Counter c) {\n        return c.total;\n    }\n\n    void step(Counter c) {\n        c.total = c.total + 1;\n    }\n}\n",
            ),
        ],
        "total",
        "Counter.java",
        &[
            ("Use.java", 5, SiteAccess::Write),
            ("Counter.java", 7, SiteAccess::ReadWrite),
            ("Use.java", 13, SiteAccess::ReadWrite),
            ("Use.java", 9, SiteAccess::Read),
        ],
    );
}

#[test]
fn csharp_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            (
                "Counter.cs",
                "namespace App\n{\n    public class Counter\n    {\n        public int Total;\n\n        void Bump()\n        {\n            Total += 1;\n        }\n    }\n}\n",
            ),
            (
                "Use.cs",
                "namespace App\n{\n    public class Use\n    {\n        void Reset(Counter c)\n        {\n            c.Total = 0;\n        }\n\n        int Show(Counter c)\n        {\n            return c.Total;\n        }\n\n        void Step(Counter c)\n        {\n            c.Total = c.Total + 1;\n        }\n    }\n}\n",
            ),
        ],
        "Total",
        "Counter.cs",
        &[
            ("Use.cs", 7, SiteAccess::Write),
            ("Counter.cs", 9, SiteAccess::ReadWrite),
            ("Use.cs", 17, SiteAccess::ReadWrite),
            ("Use.cs", 12, SiteAccess::Read),
        ],
    );
}

#[test]
fn go_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            ("go.mod", "module example.com/app\n\ngo 1.22\n"),
            (
                "model.go",
                "package app\n\ntype Counter struct {\n\tTotal int\n}\n\nfunc (c *Counter) Bump() {\n\tc.Total += 1\n}\n",
            ),
            (
                "use.go",
                "package app\n\nfunc Reset(c *Counter) {\n\tc.Total = 0\n}\n\nfunc Show(c *Counter) int {\n\treturn c.Total\n}\n\nfunc Step(c *Counter) {\n\tc.Total = c.Total + 1\n}\n",
            ),
        ],
        "Total",
        "model.go",
        &[
            ("use.go", 4, SiteAccess::Write),
            ("model.go", 8, SiteAccess::ReadWrite),
            ("use.go", 12, SiteAccess::ReadWrite),
            ("use.go", 8, SiteAccess::Read),
        ],
    );
}

#[test]
fn rust_field_sites_list_the_write_then_read_writes_then_the_read() {
    assert_writes_first(
        &[
            (
                "Cargo.toml",
                "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
            ),
            ("src/lib.rs", "pub mod model;\npub mod usage;\n"),
            (
                "src/model.rs",
                "pub struct Counter {\n    pub total: i32,\n}\n\nimpl Counter {\n    pub fn bump(&mut self) {\n        self.total += 1;\n    }\n}\n",
            ),
            (
                "src/usage.rs",
                "use crate::model::Counter;\n\npub fn reset(c: &mut Counter) {\n    c.total = 0;\n}\n\npub fn show(c: &Counter) -> i32 {\n    c.total\n}\n\npub fn step(c: &mut Counter) {\n    c.total = c.total + 1;\n}\n",
            ),
        ],
        "total",
        "src/model.rs",
        &[
            ("src/usage.rs", 4, SiteAccess::Write),
            ("src/model.rs", 7, SiteAccess::ReadWrite),
            ("src/usage.rs", 12, SiteAccess::ReadWrite),
            ("src/usage.rs", 8, SiteAccess::Read),
        ],
    );
}

const PYTHON_COUNTER: &[(&str, &str)] = &[
    (
        "model.py",
        "class Counter:\n    def __init__(self):\n        self.total = 0\n\n    def bump(self):\n        self.total += 1\n",
    ),
    (
        "use.py",
        "from model import Counter\n\n\ndef reset(c: Counter):\n    c.total = 0\n\n\ndef show(c: Counter):\n    return c.total\n\n\ndef step(c: Counter):\n    c.total = c.total + 1\n",
    ),
];

#[test]
fn a_line_that_writes_and_reads_the_field_is_one_read_write_site() {
    let repo = scanned_repo(PYTHON_COUNTER);
    let conn = open_read_only(&repo.db).unwrap();
    let identifiers: Vec<String> = conn
        .prepare(
            "SELECT json_extract(metadata_json, '$.access') FROM identifiers
             WHERE name = 'total' AND path = 'use.py' AND start_line = 13 ORDER BY start_column",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(identifiers, ["write", "read"]);

    let report = report(&repo, "total", "model.py");

    let line_13: Vec<_> = report
        .sites
        .iter()
        .filter(|site| site.site.path == "use.py" && site.site.start_line == Some(13))
        .collect();
    assert_eq!(line_13.len(), 1, "{report:#?}");
    assert_eq!(line_13[0].access, Some(SiteAccess::ReadWrite));
    assert!(
        format_reference_report(&report, 20)
            .contains("- `step` [use.py:13] (kind: member_access; receiver type; read-write)\n")
    );
}

#[test]
fn a_function_target_keeps_its_report_without_access() {
    let repo = scanned_repo(&[
        (
            "model.py",
            "class Counter:\n    def __init__(self):\n        self.total = 0\n\n    def bump(self):\n        self.total += 1\n",
        ),
        (
            "use.py",
            "from model import Counter\n\n\ndef run(c: Counter):\n    c.bump()\n    handler = c.bump\n    return handler\n",
        ),
    ]);

    let report = report(&repo, "bump", "model.py");

    assert!(!report.sites.is_empty(), "{report:#?}");
    assert!(report.sites.iter().all(|site| site.access.is_none()));
    assert_eq!(report.access_counts, None);
    let text = format_reference_report(&report, 20);
    assert!(!text.contains("writes"), "{text}");
    assert!(!text.contains("; read"), "{text}");
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.contains("\"access"), "{json}");
}

#[test]
fn a_page_of_field_sites_continues_after_the_writes() {
    let repo = scanned_repo(PYTHON_COUNTER);
    let report = report(&repo, "total", "model.py");

    let first = format_reference_report_page(&report, 1, 0);
    let second = format_reference_report_page(&report, 1, 1);
    let last = format_reference_report_page(&report, 1, 3);

    assert!(
        first.contains("- `reset` [use.py:5] (kind: member_access; receiver type; write)\n"),
        "{first}"
    );
    assert!(
        second.contains("- `bump` [model.py:6] (kind: member_access; class chain; read-write)\n"),
        "{second}"
    );
    assert!(
        last.contains("- `show` [use.py:9] (kind: member_access; receiver type; read)\n"),
        "{last}"
    );
    for page in [&first, &second, &last] {
        assert!(
            page.contains("Sites: 4 · proven 4 · likely 0 · unresolved 0 · excluded 0 · writes 1, read-writes 2, reads 1, unclassified 0\n"),
            "{page}"
        );
    }
}

#[test]
fn a_context_of_a_field_lists_its_callers_with_writes_first() {
    let repo = scanned_repo(PYTHON_COUNTER);
    let workspace = Workspace::new(repo.root.clone());
    let conn = open_read_only(&repo.db).unwrap();
    let target = select_reference_target(&conn, "total", Some("model.py")).unwrap();

    let slice = get_context_slice_selected_op(
        &workspace,
        &repo.db,
        &conn,
        &SymbolSelector::Id(target.symbol_id),
        Some("model.py"),
        false,
    )
    .unwrap();

    let callers = slice.callers.as_ref().unwrap();
    assert_eq!(callers.access_counts, Some(ONE_OF_EACH));
    let accesses: Vec<_> = callers.sites.iter().map(|site| site.access).collect();
    assert_eq!(
        accesses,
        [
            Some(SiteAccess::Write),
            Some(SiteAccess::ReadWrite),
            Some(SiteAccess::ReadWrite),
            Some(SiteAccess::Read),
        ]
    );
    let text = format_context_slices(std::slice::from_ref(&slice));
    assert!(
        text.contains(
            "### Callers\nSites: 4 · proven 4 · likely 0 · unresolved 0 · excluded 0 · writes 1, read-writes 2, reads 1, unclassified 0\nProven (4):\n- `reset` [use.py:5] (kind: member_access; receiver type; write)\n"
        ),
        "{text}"
    );
}
