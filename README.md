# code-kb

> **Interactive Showcase & Benchmarks:** [https://anortham.github.io/code-kb/](https://anortham.github.io/code-kb/)

`code-kb` is a code-intelligence engine and Model Context Protocol (MCP) server for AI coding agents.

Backed by [`julie-extractors`](https://github.com/anortham/julie-extractors),
code-kb returns file outlines, symbol bodies, reference evidence and edit checks.
Individual responses are **61–99% smaller than a full-file read** in
[measured examples](docs/reviews/2026-09-26-project-evaluation.md#what-the-token-measurements-establish).
Those comparisons measure response size. The
[completed-edit comparison](evals/edit-2026-10-27/results/report.md) measures
accepted patches, observed tokens and time against native tools.
Agents edit with their native filesystem tools; code-kb automatically refreshes
indexed files after filesystem changes.

Support covers every language reported by the pinned Julie extractor, currently 42 code, markup,
configuration, and data languages. Real-extractor fixtures check each language; advanced checks
follow its call and type semantics. See [language support and tested coverage](docs/language-support.md)
for the fixture inventory and remaining limits.

---

## Why code-kb?

When a task needs one signature or implementation, a file outline or symbol body can supply that context without loading the whole file.

`code-kb` exposes progressively more detail:

1. **Repository Orientation (`file_skeleton` on a folder):** Understand directory structures and key exports in a few hundred tokens.
2. **File Skeletons (`file_skeleton`):** Inspect function signatures, types, traits, and docstrings with implementation bodies stripped.
3. **Symbol Lookup & Discovery (`lookup_symbol`):** Instant exact/prefix identifier lookups. A query that names no symbol exactly also gets ranked conceptual matches over names, signatures, and docstrings; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`).
4. **One-Call Edit Context (`get_symbol_context`):** In a single turn, fetch a target function's body and return type, a summary of its callers with evidence labels, the tests to run with their commands, the markers its sibling symbols share, its callee signatures, its parameter types, and the open ends of a short impact walk.
5. **Fresh Indexes:** Filesystem edits automatically refresh indexed files, and target body and skeleton reads refresh their file before answering.

---

## Key Principles

- **Small Retained Memory:** Written in Rust, zero heavy runtimes (no web dashboard, no GPU models), retained process memory is about 25–40 MB, and about 50–60 MB after a callers report of a common name in a large project. The plugin launcher is a small Node script that replaces itself with the native binary via `process.execve` on supported POSIX Node 22+ runtimes (or waits via `spawn` on Windows/older Node).
- **Millisecond Query Latency:** Direct SQLite queries in WAL mode with zero in-memory heap bloat. On this repository a warm symbol body or small skeleton takes 1–4 ms, and most other calls take 10–100 ms; a concept search, a callers report, or `blast_radius` can take up to about 200 ms, and `check_edit` takes 1–5 s on a real diff.
- **One `project_root` on Every Call:** Every tool takes `project_root`, the absolute path of the project or git worktree the agent works in (`lookup_symbol(project_root="/path/to/project", query="...")`). A root in every call cannot go stale, so a move to a worktree never answers from the old index.
- **CLI-First Parity:** Every MCP tool has an exact 1:1 CLI command for instantaneous terminal verification and dogfooding.
- **Automatic Index Sync:** No tool manages the index, and none is needed. A debounced background watcher refreshes changed files and runs one bulk scan for a Git storm. A reconcile runs at startup and when a call switches to another root. Body and skeleton reads refresh their target file first.

---

## Install

Install the plugin for your agent. The plugin ships a small Node.js launcher. On the first
run it downloads the matching `code-kb` release archive for your platform, verifies its
SHA-256, and unpacks `code-kb` and `julie-extract` into `~/.code-kb/dist/<version>/`. Later
runs start instantly. Nothing else to download or put on `PATH`.

Requirements:
- Node.js 18 or newer on `PATH` (the launcher is a Node script; the server itself is a native binary).
- Windows 10 build 17063 or newer (the launcher unpacks with the built-in `tar.exe`).

### Claude Code

```text
/plugin marketplace add anortham/code-kb
```
```text
/plugin install code-kb@code-kb
```
*(Send as two separate prompts.)* The plugin registers the MCP server, the progressive
disclosure skill, a `/code-kb:code-kb-telemetry` command for usage and token-savings reports, a
`/code-kb:report-issue` command that files a bug with a diagnostic bundle, SessionStart and
SubagentStart hooks that inject routing instructions, and Stop and SubagentStop hooks that run the
edit check (see [After you edit](#after-you-edit-check_edit-and-the-stop-hooks)).

The routing instructions tell the agent to call `file_skeleton` on a folder or a file it does
not know, to call `get_symbol_context` before it changes a symbol, and to `Read` the file it
will edit, since the editor needs that read.

### Codex

```bash
codex plugin marketplace add anortham/code-kb
codex plugin add code-kb@code-kb
```
Run `codex`, open `/hooks`, and trust the four code-kb hooks.

To update, refresh the marketplace snapshot first. `codex plugin remove` followed by
`codex plugin add` reinstalls the version already in the snapshot:

```bash
codex plugin marketplace upgrade
codex plugin remove code-kb@code-kb
codex plugin add code-kb@code-kb
```

### Antigravity CLI (AGY)

```bash
agy plugin install https://github.com/anortham/code-kb
```
The plugin registers the MCP server and a `PreInvocation` hook. Antigravity has no
`SessionStart` hook, so routing directives arrive per turn through `injectSteps`.

### Grok CLI

```bash
grok plugin install anortham/code-kb --trust
```

### First Run & Automatic Indexing

You do not need to run `code-kb scan` by hand. The first `code-kb` tool call or CLI command in
a repository creates `<workspace>/.code-kb/artifact.db` and runs the initial scan. A git
worktree copies its parent repository's index instead of scanning again. Files that changed
while no session ran, for example after a branch switch, are reconciled before the first
answer. If a refresh fails, indexed reads return an error naming the project and the cause
instead of serving stale results. Fix the reported filesystem or extractor issue and retry
the call; completed updates are preserved. Pre-index a large repository before a session with:

```bash
code-kb scan
```

No MCP tool manages the index, and none is needed. The index rebuilds itself once when a
different `julie-extract` version or extraction level wrote it, or when SQLite cannot read it.
The rebuild scans into a side file and then moves it into place, so reads never find the index
missing. If a rebuild fails, the error names the command to run, `code-kb scan --root <root>`,
with the full path of the running `code-kb` binary in place of `code-kb`, because a plugin
install does not put `code-kb` on `PATH`. On Windows the error gives the command for
PowerShell (`& '<code-kb>' scan --root '<root>'`) and for Git Bash. `code-kb scan --force`
forces a full re-extraction.

The index honors `.gitignore`, `.ignore`, `.julieignore`, `.code-kb-ignore`, and
`.codekbignore`, and applies changes to them automatically. These folders are never indexed,
and no ignore rule adds them back: `.git`, `.hg`, `.svn`, `.julie`, `.miller`, `.code-kb`,
`.memories`, `.agents`, `.razorback`, `.worktrees`, `worktrees`, `.claude`, `.venv`, `venv`,
`.env`, `.tox`, `.vs`, `node_modules`, `vendor`, `target`, `dist`, `build`, `.cache`, `obj`,
`TestResults`, `.idea`, and `.vscode`. Files whose names end in `.min.js`, `.bundle.js`,
`.generated.js`, `.generated.jsx`, `.generated.ts`, `.generated.tsx`, `.generated.d.ts`,
`.tmp`, `.swp`, `~`, `.db-wal`, `.db-shm`, `.sqlite-wal`, or `.sqlite-shm` are never indexed.
`julie-extract` skips a source file larger than 1 MiB.

Launcher environment variables:

| Variable | Effect |
|---|---|
| `CODE_KB_HOME` | Directory that holds `dist/` (default `~/.code-kb`). |
| `CODE_KB_VERSION` | Release version to fetch instead of the plugin's own version. |
| `CODE_KB_BIN` | Run this binary and skip the download entirely (local builds). |

Two overrides need no environment variable, which matters in harnesses that do not pass the
environment to MCP servers (Codex):

- A binary or symlink at `~/.code-kb/bin/code-kb` (`code-kb.exe` on Windows) runs instead of any
  download, in every harness. For development: `ln -s /path/to/code-kb/target/release/code-kb ~/.code-kb/bin/code-kb`.
- A plugin installed from a source checkout runs that checkout's `target/release/code-kb` when it
  exists.

With either in place, `cargo build --release` plus a session restart is the whole development loop.

### Uninstall

| Harness | Command / Action |
|---|---|
| Claude Code | `/plugin remove code-kb` |
| Codex | `codex plugin remove code-kb@code-kb` |
| Antigravity (AGY) | `agy plugin uninstall code-kb` |
| Grok CLI | `grok plugin uninstall code-kb` |

Delete `~/.code-kb/dist` to remove the downloaded binaries. Each workspace keeps its index
in `<workspace>/.code-kb/`; delete that directory to remove the index.

---

## Manual Configuration

Use this path for harnesses without a plugin manager, or when you would rather manage the
binary yourself.

### Step 1: Get the Binaries

- **GitHub Releases:** download the archive for your platform from
  [GitHub Releases](https://github.com/anortham/code-kb/releases):
  - Linux x86_64 and ARM64 (`.tar.gz`)
  - macOS Apple Silicon and Intel (`.tar.gz`)
  - Windows x86_64 and ARM64 (`.zip`)

  Unpack it and put both binaries on your `PATH` (for example `~/.local/bin`,
  `/usr/local/bin`, or `C:\tools`). `code-kb` and `julie-extract` are packaged side by side,
  and `code-kb` finds `julie-extract` next to its own executable.
- **Cargo:**
  ```bash
  cargo binstall code-kb-cli
  # or
  cargo install code-kb-cli
  ```
  Cargo installs `code-kb` only. Download the pinned `julie-extract` from the
  [julie-extractors releases](https://github.com/anortham/julie-extractors/releases) (version
  in `scripts/julie-pins.json`) and put it on your `PATH` or set `JULIE_EXTRACT_BIN`.
- **Verify:**
  ```bash
  code-kb --version
  ```

### Step 2: Register the MCP Server

Every harness runs the same command: `code-kb serve`. Hooks run `code-kb hook <Event>`.

#### How code-kb Finds Your Project

Every tool takes a required `project_root`: the absolute path of the
project or git worktree the agent works in. The agent sends the same value on every call. It
changes the value when it moves to a worktree or another project. The routing hook and the skill
tell the agent to do this.

- A plain path or a `file://` URI is accepted. A relative value is an error.
- A subfolder or a file inside the project resolves to the enclosing project.
- `code-kb` refuses a filesystem root (`/`, `C:\`), the home directory, and a folder with no
  project marker (`.git`, `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`) and no
  code-kb index. The error names the path and the reason, and `code-kb` creates nothing.
- `path` and `file_path` are relative to `project_root`, or absolute inside it. An absolute path
  outside `project_root` is an error. A relative or absolute path inside a nested git worktree or
  submodule counts as outside `project_root`. The error names that nested root. A path never switches the
  project.
- A project with no index gets one on the first call. A git worktree copies its parent
  repository's index; any other project runs a full scan. If the index is not ready in 5 s, the
  answer is `Indexing <root> started; call again in a few seconds.`

The server keeps one active index. A call for another project switches to that project's
`<root>/.code-kb/artifact.db`.

`--root` on the `serve` command is optional for every client, including GUI apps. It only names
the startup pre-warm root: the server prepares that project's index at start. Without `--root`,
the server pre-warms the directory the process starts in. `--db <file>` pins the index file of
that launch root only.

The CLI takes the same value as the global `--root` flag, which defaults to the current directory.
It refuses a home directory or a filesystem root the same way the tools do. `code-kb scan` runs on
any root you give it.

#### Claude Code (without the plugin)

```bash
claude mcp add --scope user code-kb -- code-kb serve
```

#### Codex (without the plugin)

In `~/.codex/config.toml`:
```toml
[mcp_servers.code-kb]
command = "code-kb"
args = ["serve"]
```

#### Antigravity CLI (without the plugin)

```bash
agy mcp add code-kb code-kb serve
```

Global config (`~/.gemini/config/mcp_config.json`) with `"eager": true`. The same config works
for the AGY CLI and the Antigravity IDE:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"],
      "disabled": false,
      "eager": true,
      "force_all_tools_eager": true
    }
  }
}
```

Lifecycle hook configuration (`~/.gemini/config/hooks.json`):
```json
{
  "code-kb": {
    "PreInvocation": [
      {
        "command": "code-kb hook PreInvocation",
        "timeout": 10,
        "type": "command"
      }
    ]
  }
}
```

Progressive disclosure skill linking:
```bash
ln -sf /path/to/code-kb/skills/code-kb ~/.gemini/config/skills/code-kb
```

#### Grok CLI (without the plugin)

Project-level `.mcp.json`:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

#### Cursor

In `.cursor/mcp.json` at the repository root:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

#### OpenCode

Add to `opencode.json` in the project:
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

#### Claude Desktop

Claude Desktop has one global config. The agent passes `project_root` on each call, so one server
works for every project.
Add to `claude_desktop_config.json` (`%APPDATA%\Claude\claude_desktop_config.json` on Windows, `~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):
```json
{
  "mcpServers": {
    "code-kb": {
      "command": "code-kb",
      "args": ["serve"]
    }
  }
}
```

#### Other GUI Apps (Windsurf, Visual Studio, VS Code)

Use the app's MCP config file and the same arguments: `["serve"]`. To pre-warm one project's
index at start, add `"--root", "/absolute/path/to/project"`. On Windows write the path with
forward slashes, for example `C:/source/project`.

#### GitHub Copilot CLI & Terminal Agents

For terminal harnesses (Copilot CLI, Pi, Swival, Zed), configure the MCP server to run
`code-kb serve`.

To remove a manual configuration, delete the `code-kb` entry from the harness config
(`claude mcp remove code-kb`, `agy mcp remove code-kb`, or edit the file) and delete the
binaries from your `PATH`.

---

## Qt and QML

For a Qt developer, `code-kb` answers the everyday questions about a QML code base.
`file_skeleton` prints the component as its object tree: declared properties, signals,
functions, inline components, and every nested object under its owner.
`find_references` crosses component files: it lists the components that extend a base
type as `extends` rows, the plain and qualified instantiations (`Kirigami.Page` as well
as `Page`), the files that read a singleton, and the signal handlers, each labelled
`handler` when the receiver names the owner and `handler (candidate)` when it does not.
`qmldir` module files and `.qmltypes` type descriptors are indexed. Qt JavaScript files
parse, including the `.pragma library` and `.import` directives. KDE test files under
`autotests/` and files named `tst_*.qml` are hidden from
search by default; `--include-tests` shows them.

Source builds use the julie-extract version pinned in [scripts/julie-pins.json](scripts/julie-pins.json). QML references retain inline component
ownership, respect shadowed `id` names, and resolve `parent` and `this` from their
enclosing object. QML `import` directives appear as structural facts. Qt JavaScript
`.import` symbols preserve their exact spans, so body reads exclude trailing comments.
`code-kb facts pragma --path src` lists `.pragma` facts from both QML and Qt JavaScript.

A Qt C++ header indexes the same way. `file_skeleton` prints one `Q_PROPERTY(...)` row
per declared property under its class, keeping the `READ`, `WRITE`, `NOTIFY`, and
`MEMBER` accessors the macro names. A method declared in a `Q_SIGNALS:` section is an
`event` row, and the skeleton marks it `// event` before its line range because its C++
signature reads like a method. A method in a `Q_SLOTS:` section carries `qt_slot`, and a
`Q_INVOKABLE` method carries `qt_invokable`. A class records the QML element name it declares with
`QML_ELEMENT` or `QML_NAMED_ELEMENT`. A forward declaration such as `class ColumnView;`
emits no row, so `lookup_symbol` of a class name returns one row, at its definition.
`code-kb facts` takes a `property` alias that covers the QML property
declarations and the Qt C++ `Q_PROPERTY` facts together, each fact naming its class.
Property facts expose their available Qt metadata, including `property_type`, accessors,
and the optional `designable`, `scriptable`, `stored`, `user`, and `revision` attributes.
Until upstream header updates preserve C++ language detection, one batch of changed `.h`
files triggers a content-aware workspace scan that only re-extracts changed files.

First-class Qt support: QML, `qmldir`, `.qmltypes`, Qt JavaScript, and Qt C++ headers,
validated on pinned corpus revisions; static reference results have documented limits;
`.ui` and CMake files are not indexed.

### Historical 1.6.0 / Julie 3.3.0 measurements

| Corpus | Pinned commit | QML files | C++ files |
| :--- | :--- | ---: | ---: |
| Omarchy shell (Quickshell) | `49306774` | 106 | 0 |
| KDE Kirigami | `ca7d636` | 225 | 102 |
| KDE plasma-workspace | `a45871a` | 222 | 1051 |
| Quickshell examples | `c6d1236` | 14 | 0 |

These corpus revisions and examples were measured with code-kb 1.6.0 and julie-extract
3.3.0. They are historical measurements, not 3.3.1 results.

```bash
# Omarchy
code-kb skeleton shell/Ui/Button.qml
# 71 lines for a 209-line file; the object tree nests, so ToolTip, Row, MouseArea,
# and HoverHandler hold their own children.

code-kb refs BarWidget --file shell/Ui/BarWidget.qml --limit 200
# 15 rows: 12 `extends` rows, one per component that extends BarWidget.
# The other 3 rows are signal handlers, grouped one row per file.

code-kb refs Color --file shell/Commons/Color.qml --limit 200
# 53 rows, one per file that reads the Color singleton.

code-kb refs clicked --file shell/Ui/Button.qml
# 20 rows: 4 emit sites, 1 `handler` row, and 15 `handler (candidate)` rows.

code-kb lookup SpeedDial
# 1 row: class SpeedDial [shell/Ui/SpeedTestOverlay.qml:206-411],
# signature `component SpeedDial: Item`, an inline component.

# Kirigami
code-kb refs Page --file src/controls/Page.qml --limit 200
# 60 rows, of which 7 are `extends` rows (four of them inline components).

# Kirigami, Qt C++
code-kb skeleton src/layouts/columnview.h
# 231 lines for a 785-line file, 38 `Q_PROPERTY(...)` rows, no parse-error banner.

code-kb lookup ColumnView
# 1 class row: `class ColumnView [src/layouts/columnview.h:276-783]`.
# The forward declaration in the same file adds no second row.

code-kb facts property --path src/layouts/columnview.h --limit 5
# 5 of the file's 38 `cpp.qt_property.v1` facts, each naming the class that owns it
# (`ScrollIntentionEvent`, `ColumnViewAttached`).
```

### Known limits

- A project macro that the extractor does not know is not blanked before the parse, so it
  can still cause a parse diagnostic. Diagnostics remain on both C++ corpora: 31 on
  Kirigami and 298 on plasma-workspace, down 97% and 94% from julie-extract 3.2.0. None
  of them is macro-shaped; a file-local `#define` used as a statement accounts for 12 of
  Kirigami's 31.
- `Q_DECLARE_FLAGS(Modes, Mode)` and a `Q_OBJECT_BINDABLE_PROPERTY(...)` member are
  blanked as whole statements, so the typedef and the member emit no row.
- A macro used inside a line, such as `Q_ARG(bool, true)` in a `QMetaObject::invokeMethod`
  call, is left alone by design, because rewriting it would delete real arguments.
- `.ui` designer files and CMake files are not indexed.
- A QML module imported through an alias resolves to a workspace file only when the alias
  is not a Qt module. `QtQuick.*` and `QtQml.*` aliases never name a workspace symbol.
- Reference pages contain at most 200 rows. Use `offset` to retrieve more rows. For a heavily
  used singleton, `refs` groups member accesses per file, one row per file with a count.

---

## MCP Tool Catalog

Every tool requires `project_root`, the absolute path of the project or
git worktree you work in. The table does not repeat it. `path` and `file_path` are relative to
`project_root`, or absolute inside it.

| Tool | Purpose | Key Parameters | Aliases |
| :--- | :--- | :--- | :--- |
| `file_skeleton` | File outline with function & method bodies stripped (61–90% smaller than the file in measured examples); a directory returns its layout and main exports. A skeleton over about 8K tokens (32,000 characters) keeps its outer rows and folds the deepest ones into `/* N more rows: Lx-Ly */` comments. A folder outline over that size names fewer definitions per file, then fewer levels. `public_only` hides private, protected, and fileprivate rows and counts them. | `file_path` (req), `public_only` (opt, def: false) | `file`, `path` (for `file_path`) |
| `lookup_symbol` | Find symbols by name, qualified name (`Type::method`), prefix, or concept. A query that names no symbol exactly also lists the ranked matches under `No exact name match; N full-text matches`. The ranking credits each query term once from its strongest field (name, signature, docstring, then a small credit for the row's module name), weighted by the term's rarity across the index, plus kind and path priors; substrings inside identifiers are found (`sha256` finds `parseSha256Sidecar`). When nothing matches, the answer names the workspace and lists up to three similar names, definitions before imports; a one-word query that no row names exactly lists them before the ranked matches too (CLI `lookup --json`: `suggestions`). A query with `/`, `.`, `{`, or `:` also lists the routes and config keys it names (``route `GET /users/<int:id>` (app.py:6) -> show_user``, ``config `mcp_servers.code-kb.command` (config.toml:2)``). When the query names one definition (functions, methods, and types; imports fold) and its body is under a quarter of the 8K-token budget, the answer ends with that body as `get_symbol_body` prints it (CLI `lookup --json`: `body`); a longer body gets a line with the `symbol_id` to pass to `get_symbol_body`. Test functions, test containers, and rows from test files are hidden unless `is_test` is true, except a row whose name equals the query. | `query` (req), `path` (opt), `kind` (opt), `is_test` (opt, def: false), `limit` (opt, def: 20) | `name`, `q`, `symbol_name`, `symbol` (for `query`); `file_path`, `file` (for `path`); `include_tests` (for `is_test`) |
| `get_symbol_body` | Slices the source of a symbol from disk as written, with its declaration and decorators; a name with overloads in one class returns each overload. A text answer over about 8K tokens shows a type's members with their bodies hidden, or the first lines of other source, and names the lines to read for the rest; CLI `--json` keeps the whole body. | Exactly one `symbol_name` or current-index `symbol_id`; `file_path` (opt, a file or a folder) | `symbol`, `name`, `target` (for `symbol_name`); `file`, `path` (for `file_path`) |
| `get_symbol_context` | Edit bundle: target body + return type + callers summary (`Sites:` line, first proven and likely sites) + tests to run with commands + callee signatures + parameter types + open ends of a depth-2 walk; a line under `### Callers` names the `find_references` and `blast_radius` calls, with the `symbol_id`, for the full lists; up to four overloads in one class return a bundle each, and more return an error that lists each overload's id. A target body over about 8K tokens is cut as in `get_symbol_body`. | Exactly one `symbol_name` or current-index `symbol_id`; `file_path` (opt, a file or a folder), `include_external` (opt, def: false) | `symbol`, `name`, `target` (for `symbol_name`); `file`, `path` (for `file_path`) |
| `find_references` | Callers: every indexed site of the name (a target in a language that records no reference sites, such as JSON or Markdown, gets a `Coverage:` line that says so), each labeled proven, likely, unresolved, or excluded with its evidence and, for a call, its argument count (a count that no definition of the target accepts excludes a `likely` or `unresolved` call in a language with a strict argument rule; [docs/argument-counts.md](docs/argument-counts.md) lists the languages and the rules), plus a whole-word text count that accounts for every line of the name. The sites of a field, property, variable, or constant show `write`, `read-write`, or `read`, writes first in each label, and the `Sites:` line counts them. After the sites, up to six sections list what else a change may reach: `Overrides` and `Overrides of` (same-named methods in subtypes and in ancestors), `Constructions` (lines that build a class, struct, or record), `Variant uses` (functions that name variants of an enum), `Registrations` (route facts that register a function as a handler), and `Tests that call or use it` (each with the command that runs it). The sections never change a label or a count; CLI `refs --json` adds them as arrays. Callees: the calls a symbol makes (filters external stdlib noise). Qualify overloaded names. | Exactly one `symbol_name` or current-index `symbol_id`; `file_path` (opt), `direction` ("callers" \| "callees", def: callers), `offset` (opt, def: 0), `limit` (opt, def: 20, listed rows only), `include_external` (opt, def: false) | `symbol`, `name`, `target` (for `symbol_name`); `file`, `path` (for `file_path`) |
| `blast_radius` | Traces callers across multiple steps. A target in a language that records no reference sites gets a `Coverage:` line that says so. Lists the tests the call graph reaches; when the walk finds none for one symbol, it lists the tests of its callers report, or the target itself when it is a test. Incomplete coverage names a broader configured test command when project files identify one; otherwise it names the broader scope to verify. A larger `depth` may find more tests. The graph discovery ceiling remains 200 rows. With no `symbol`, `symbol_id`, or `file`, it walks the uncommitted git changes. | `symbol` or current-index `symbol_id` (opt), `file` (opt: seeds a walk from every symbol of the file, narrows a `symbol` name, or guards a `symbol_id`), `depth` (opt, def: 2, at most 5), `offset` (opt, def: 0), `limit` (opt, def: 20), `include_low_signal` (opt, def: false) | `symbol_name`, `name`, `target` (for `symbol`); `file_path`, `path` (for `file`); `max_depth` (for `depth`) |
| `check_edit` | After you edit: the symbols whose shape changed against the session base (or `base`, or `HEAD`) and every site of each that your diff did not reach. Run it before you say a change is done. | `base` (opt, a git revision), `offset` (opt, def: 0), `limit` (opt, def: 12) | |

Pages use the current index. Restart at offset 0 after editing or rebuilding. `offset` applies
to each displayed section independently; counts describe the complete report, subject to any
explicit graph or edit-check cut. CLI `refs`, `blast-radius`, and `check` take the same `--offset`
and `--limit` controls. `blast-radius --include-low-signal` exposes discovered imports, fields,
properties, variables, parameters, modules, and namespaces that the default view hides.

Four query commands have no MCP tool: `outline` (a folder's layout; `file_skeleton` on a folder returns the same), `search` (the ranked search with `--explain`), `facts` (routes, SQL queries, models, and config keys), and `stats` (token savings and tool errors).

### After you edit: `check_edit` and the Stop hooks

`check_edit` (CLI `code-kb check [--base REV]`) compares the working tree with a base revision:
`base`, else the commit the session started at, else `HEAD`. It lists each symbol whose shape
changed (removed or renamed, new parameters or signature, new or lost fields, new or lost enum
variants) and the sites of it that your diff did not reach, with the tests to run:

```text
check_edit: working tree against fb5628e809 (HEAD), 2 files changed
1 shape change:
  function `area` (src/geometry.rs): parameters 2 → 3
    In your diff (1): src/lib.rs:4
    Not in your diff, likely (1):
      src/lib.rs:8  `second`  passes 2 args; takes 3
Not checked: calls inside macro arguments, callback invocation contracts. Indexed callable values are advisory dependencies.
```

- `code-kb hook SessionStart` records the session base in
  `<project>/.code-kb/sessions/<session_id>.json`, also when the project has no index yet. That
  folder is inside `.code-kb/`, which git ignores.
- `code-kb hook Stop` and `code-kb hook SubagentStop` run the check when the agent stops. The
  Claude Code and Codex plugins wire them. If a `proven` site is outside the diff, the hook
  blocks the stop and lists up to 15 sites. A `likely` site, a `signature changed` row, and a
  JavaScript parameter change never block; `check_edit` still lists them. It stays silent when
  nothing changed shape,
  for the diff it checked last or blocked on last, when `stop_hook_active` is true,
  outside a git repository, when another `julie-extract` wrote the index, and on any error.
- A check reads at most 200 files and 30 symbols and starts no new step after 20 s. The answer states each cut.
- The rules and the known limits are in [docs/edit-check.md](docs/edit-check.md).

---

## CLI Commands (Direct Terminal Usage)

Every MCP capability can be executed directly from your terminal with 1:1 parity:

```bash
# Architectural outline of current repo (depth 2)
code-kb outline

# Skeleton of a specific file (implementation bodies stripped)
code-kb skeleton src/main.rs

# Lookup symbols by name, prefix, or concept (alias: code-kb symbol)
code-kb lookup Workspace --kind struct --path crates/code-kb-core

# The ranked search that lookup adds when no name matches (sha256 finds parseSha256Sidecar)
code-kb search "syntax validation concurrency"

# Same search with the rerank breakdown under each result (JSON: an `explain` object per result)
code-kb search "sha256 sidecar" --explain

# Retrieve exact implementation body of a symbol
code-kb body Workspace

# Before an edit: body, return type, callers summary, tests to run, callees, types, open ends (alias: code-kb slice)
code-kb context ensure_fresh_file

# Find callers, each labeled proven, likely, unresolved, or excluded, with a text count
code-kb refs open_read_only --direction callers

# View callees including external standard library tokens
code-kb refs open_read_only --direction callees --include-external

# Predict blast radius and targeted tests to run for a symbol or file
code-kb blast-radius compute_blast_radius

# Zero-argument blast radius: auto-discovers uncommitted git working-tree changes
code-kb blast-radius
# (alias: code-kb impact)

# After you edit: the symbols whose shape changed and the sites your diff did not reach
code-kb check
code-kb check --base main
code-kb check --fail-on-proven   # exit 3 when a proven site is outside the diff

# Query framework structural facts (or omit category to list all detected categories)
code-kb facts
code-kb facts config --path Cargo.toml
code-kb facts route --limit 10

# Print the log folder and the latest lines of the newest log file (it does not stream).
# A server logs to <launch root>/.code-kb/logs (the --root of `serve`, or the project of the
# folder it started in) for every project it serves; a server started outside a project logs
# to ~/.code-kb/logs.
code-kb logs

# Token savings, call counts, and error rates (alias: code-kb telemetry)
code-kb stats --since month
code-kb stats --workspace

# Self-contained diagnostic bundle (versions, index facts, recent errors, log tail) with a
# pre-filled GitHub issue link; the /report-issue skill files it through `gh issue create`
code-kb bug-report --title "lookup returns nothing" --description "what went wrong"

# Output agent lifecycle hook payload (SessionStart / SubagentStart / PreInvocation)
code-kb hook SessionStart
code-kb hook SubagentStart
code-kb hook PreInvocation
# Stop and SubagentStop read the hook JSON on stdin and run the edit check;
# SessionStart reads it too and records the session base
code-kb hook Stop
code-kb hook SubagentStop
```

---

## Telemetry and What "Saved" Means

`code-kb` records every MCP tool call in `~/.code-kb/telemetry.db`, with the name the client
gives itself. `code-kb stats` reports the calls, the latency, the error rate, the tokens served,
and the tokens saved. The Stop hooks record each check, each block, and whether the agent edited a
blocked site (see [docs/edit-check.md](docs/edit-check.md)). No row holds source text.
`CODE_KB_NO_TELEMETRY=1` turns all of it off.

Saved is the size, in estimated tokens, of the files the answer points into, minus the tokens
served. A skeleton, body, or context read is measured against its own file. A lookup,
references, or blast-radius answer is measured against the distinct files its rows name,
at most 20 files. A call with no file to point at, such as a folder skeleton,
records no baseline and is not counted as saved.

The report states that coverage beside the number, so you can see how much of the window it
covers:

```text
Tokens Saved, upper bound: ~<saved> (baseline known for <K> of <M> calls; baseline = reading each file an answer names in full)
```

Each tool row carries the same pair as `~N (K/M)`. Rows written before this measurement existed
carry no baseline and are never rewritten.

---

## Development

For contributors building `code-kb` from source:

### Prerequisites
- [Rust](https://www.rust-lang.org/) (1.95+ / Edition 2024)
- Extractor binary: Run `./scripts/restore-julie-extract.sh` to download the pinned [`julie-extract`](https://github.com/anortham/julie-extractors) binary

### Build and Install Locally
```bash
# Restore pinned julie-extract binary
./scripts/restore-julie-extract.sh

# Install code-kb binary from local checkout
cargo install --path crates/code-kb-cli --force
```

### Running Tests & Verification
```bash
# Run test suite
cargo test --workspace

# Run the plugin launcher and manifest tests
node --test tests/plugin/*.test.cjs

# Run release pre-flight verification
./scripts/release-preflight.sh
```

To run the Claude Code plugin from this checkout, build and load it. The launcher runs
`target/release/code-kb` when it exists, so no download happens:
```bash
cargo build --release
claude --plugin-dir .
```

---

## Architecture & Engineering Plans

`code-kb` was designed based on extensive benchmarking and retrospectives from earlier code intelligence engines:

- [**001: Architecture & Service Model**](docs/plans/001-architecture-and-service-model.md) — Multi-project workstation scope, Git worktree deduplication, and RAG vs. AST evaluation.
- [**002: Workspace Scoping & MCP**](docs/plans/002-workspace-scoping-and-mcp.md) — 1:1 session binding, eliminating workspace registries and `workspace_id` friction.
- [**003: Tool Catalog & Schema**](docs/plans/003-tool-catalog-and-schema.md) — Historical tool-catalog decisions and token-minimized output formats.
- [**004: File Synchronization & Watchers**](docs/plans/004-file-synchronization-and-watchers.md) — The original 3-tier sync plan: tool-driven updates, JIT staleness guards, and debounced background watching. The tool-driven tier left with the source editing tools; no current tool writes files or manages the index.
- [**005: Cold-Start Reconciliation**](docs/plans/005-startup-reconciliation.md) — Detecting and reconciling offline edits in under 50ms on startup.
- [**006: Retrospective Lessons from Miller**](docs/plans/006-lessons-from-miller.md) — Analysis of calibration data, performance ledgers, and traps to avoid.
- [**010: Master Implementation Plan**](docs/plans/010-master-implementation-plan.md) — The phased engineering roadmap from workspace scaffolding to release.
- [**ADR 001: Zero Workspace Parameters**](docs/decisions/001-zero-workspace-parameters.md) — The old rule against workspace parameters in tool schemas. The required `project_root` parameter replaced it in 2.1.0.
- [**AGENTS.md Guidelines**](AGENTS.md) — Strict architectural invariants and rules for AI coding assistants working in this repository.

---

## License

Dual-licensed under MIT or Apache-2.0.
