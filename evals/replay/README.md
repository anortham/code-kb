# Edit replay cases

`scripts/edit_replay.py` replays `code-kb check` and the Stop hook on real commits, with no model
run. It measures two things:

- How often the Stop hook blocks a diff that needs no more edits (a false block).
- How many of the sites that an edit must reach the check lists.

The harness calls only the `code-kb` CLI and git, with `CODE_KB_NO_TELEMETRY=1`, so its Stop hooks
add no rows to `~/.code-kb/telemetry.db`. It works in full-history clones under
`~/.cache/code-kb-accuracy/phase3/repos/<name>`, one per project in `evals/reach/corpus.json`.
Each run checks out commits in its own detached worktrees under `<out>/trees/`, so two binaries
never share an index. Remove a run folder with `rm -r <out>` and then `git -C <clone> worktree prune`.

## Split

A commit is dev when `int(sha[:8], 16) % 2 == 0`, else holdout. The split is fixed when a case is
selected. Tune rules on dev cases only. `run` and `score` take the dev half unless `--split` names
another, and `score` on the holdout half prints counts only.

## Clean set (`clean.json`)

Commits from the corpus projects whose diff changes the shape of a symbol: a removal, a rename,
parameters, fields, or variants. The author's commit is the truth, so the diff is complete unless
a hand check finds a site that still uses the old shape.

```
scripts/edit_replay.py select clean [--per-repo 15] [--max-walk 400] [--code-kb PATH]
scripts/edit_replay.py run clean --code-kb PATH --out DIR [--split dev|holdout|all]
scripts/edit_replay.py score clean --run DIR [--split dev|holdout] [--todo FILE]
scripts/edit_replay.py verdicts clean --from FILE
```

- `select` walks back from the pinned commit of each project, skips merge commits, and keeps a
  commit when `code-kb check --base <parent>` reports at least one shape change. It stops a project
  at `--per-repo` such commits or after `--max-walk` commits. The selecting binary is recorded in
  `selected_with`. A later binary that finds a shape change in a commit the selection skipped does
  not add that commit.
- `run` records, for each case, the check JSON (the sites trimmed to path, line, and caller), the
  wall time, and the exit code. A check that fails is an error, never a clean commit. When a change
  has a `proven` or `likely` site, `run` also runs `code-kb hook Stop` with a session whose base is
  the parent, and records its decision. A hook that did not write `checked_diff` to its session
  file did not run its check, and the case is an error.
- `score` prints per language: commits, errors, commits with a shape change, hook blocks, listed
  blocks, the false ones of each, and their share of the shape commits, then p50 and p95 of the
  check time. A hook block is the binary's real Stop decision. A listed block is any `proven` or
  `likely` row outside the diff, which is what `check_edit` lists (the Stop rule before 2.7.0).
- A block is true when at least one of its rows is true, false when a hand check finds every row
  false, and unjudged otherwise. A row is true when its site still uses the old shape at that
  commit: a later commit fixed it, or the commit does not build.
- The mark: at most 2% of the shape commits get a false hook block. `Result` is `INCOMPLETE` while a
  case is an error or a hook block is unjudged.
- `score --todo FILE` writes the unjudged listed rows. A reviewer judges each one and writes a list
  of `{commit, path, line, display, change, label, verdict, evidence}` with `verdict` `true` or
  `false`. `verdicts clean --from FILE` merges them into the cases; a new verdict for the same
  path, line, and symbol replaces the old one.

## Holdback set (`holdback.json`)

Commits that change a definition in one file and its uses in other files. The selector reads git
diffs only and never calls `code-kb`, so the set does not depend on what the check already sees.

```
scripts/edit_replay.py select holdback [--per-repo 40] [--per-symbol 3] [--per-commit 3] [--max-walk 3000]
scripts/edit_replay.py select holdback --keep evals/replay/holdback.json [--out FILE]
scripts/edit_replay.py run holdback --code-kb PATH --out DIR [--split dev|holdout|all]
scripts/edit_replay.py score holdback --run DIR [--split dev|holdout] [--todo FILE]
scripts/edit_replay.py verdicts holdback --from FILE
```

- A removed line of a code file holds a definition when it matches the definition head of its
  language: `def` and `class` (Python); `function`, `= function name(`, `class`, `interface`,
  `enum`, and `type X =` (JavaScript and TypeScript); a method head or `class`, `interface`,
  `enum`, `record`, `struct` (Java and C#); `func` and `type` (Go); `fn`, `struct`, `enum`,
  `trait`, `type`, `union` (Rust). A method of a JavaScript or TypeScript class is not read.
- The definition changed when the diff removes it from the file, or when the added head of a
  callable with that name has other parameter text. In every language but Python and C#, where a
  caller can pass an argument by its name, the parameter names are blanked first, so a rename alone
  is no change. A type counts only when the diff removes it. A definition whose head spans lines is
  seen only when its first line changes, and a head whose type parameters go on to the next line
  (`function pick<`) is neither removed nor changed.
- The truth is the removed lines of other files in the same language family (JavaScript and
  TypeScript are one family) that use the name and whose use the commit changed: the name with
  its argument text (or the bare name) is on fewer added lines than removed lines of that file.
  A line that changed for another reason keeps its use and is not truth. Comment lines are not
  truth, and neither is a line inside a block comment, a Python triple-quoted string, or a Go raw
  string (a template), a name
  inside a string literal, a line that assigns the name (a local), or a key, label, or named
  argument (`{name: v}`, `f(name: v)`). For a parameter change, only a line that calls the name or
  reads it as a member is truth; an import or a type use needs no edit. A file that defines the name itself in the
  parent gives no truth, since its uses name its own definition; so an override in another file is
  not truth either. A Python, JavaScript, or TypeScript file that imports the name only from sources
  that cannot name the defining file gives no truth. A JavaScript or TypeScript source can name it
  when it is a package that names its module, or a relative path that resolves to the file or to a
  folder or `index` file above it, so a generated copy of a folder (zod's `deno/lib`) is apart from
  its source. A JavaScript or TypeScript function or type statement (not `x.name = function name(`)
  reaches another file only through an import: a bare use counts when such an import names it, and
  `recv.name` when `recv` is bound by such an import of the whole module or by any package import.
  A bare call of a Java method (not a constructor) counts only when the file imports the name
  statically (`import static p.C.name;` or `p.C.*`) from a class of the defining file, or a type the
  file extends, implements, or builds an anonymous class of reaches a class of the defining file
  through the `extends` and `implements` clauses of the project's files; a library base such as
  JUnit's `TestCase` reaches none. A use after a receiver or `::` always counts.
  A dunder name (`__init__`) makes no case, since its callers name the class. Truth lines carry their
  line numbers in the parent.
- `--keep FILE` recomputes the truth of the cases in FILE with the current rule and keeps each
  case's id, split, and verdicts, so a truth rule fix does not change the set. It prints the
  truth-line counts that changed per language and split, never a site.
- A case is the commit, the defining file, and the name. A file renamed by the commit is skipped.
  A name gets at most `--per-symbol` cases per project, and a commit at most `--per-commit`, so one
  large refactor cannot fill the set.
- `run` checks out the parent, applies the commit's change to the defining file only, and runs
  `code-kb check --base <parent>`. A truth site is caught when a `proven` or `likely` row of any
  change in the defining file has its path and line, since the check sends the edit there (a member
  of a removed class is listed under its class). A line of an import or `use` statement that spans
  lines is caught by a row on the statement's first line. Each miss gets one cause: `shape not seen` (the
  check reports no change of that name, and no cut, or the name has no kind the check compares),
  `cut by a limit` (a cut, and the name is a function, method, constructor, class, struct,
  interface, trait, enum, or record), or, from `code-kb refs` of the name on the parent tree,
  `site not indexed`, `import not listed` (an import line, which the callers report never lists),
  `site labeled unresolved`, `site excluded`, `not a call` (a parameter or
  signature change lists only calls), `fits the new shape` (the call's argument count fits the new
  range, it passes no renamed parameter by name, or a removed overload leaves another, so the check
  drops it), or `other`.
- `score` prints per language: truth sites, the ones caught, the misses, the unresolved counts,
  cases with no shape change found, and the listed rows of the changes of that name outside the
  truth. A row outside the truth is true when its site refers to the changed definition (not
  another one with that name).
  `--todo` writes at most 50 unjudged rows.
- The marks: a catch share of at least 80% in each language with at least 10 truth sites, and at
  least 90% of the judged rows outside the truth true.

## Session set (outside the repository)

Missed-site events from the owner's Claude Code and Codex session logs, taken from the
2026-10-17 direction research (`examples.json` and `errclass.txt`). Session logs are private: the
case file and every run stay outside the repository, and only aggregate counts may enter it.

```
scripts/edit_replay.py select sessions --examples FILE --errclass FILE --out DIR
scripts/edit_replay.py run sessions --cases DIR/cases.json --code-kb PATH --out DIR [--split dev|holdout|all]
scripts/edit_replay.py score sessions --run DIR [--split dev|holdout]
```

- The `verified` group is the 24 `errclass.txt` cases: a compile error of a shape change (Rust
  E0061, E0063, E0004; C# CS7036, CS1501; TypeScript TS2739) in a file the session did not edit.
  The other groups keep their names from `examples.json`. A session is dev when the SHA-256 of
  its log path passes the split rule.
- `run` rebuilds the tree just before the failing check: the session's start commit plus every
  edit that succeeded before the failing line (Claude `Edit`, `MultiEdit`, `Write`; Codex file
  changes). The start commit is Codex's recorded `commit_hash`, else the newest commit before
  the session start on its branch or on any branch where every edit applies. The session's tree is
  the checkout that holds most of its edits, which can be a worktree inside the folder where the
  session started, or another repository; then the newest commits of the branches named like that
  worktree come first. The repository is a `git clone --shared` of the session's main repository,
  so the owner's checkouts never change.
- A `cat > file <<'MARK'` or `cat >> file` heredoc in a shell command is an edit too, in its place
  among the others, unless its marker is not quoted and its body holds `$`, a backtick, or a
  backslash, which the shell rewrites.
- A case is not rebuilt, with its cause, when the log or the repository is gone, a command before
  the failing line changed the git state of the session tree (`checkout`, `reset`, `stash`,
  `rebase`, `merge`, `pull`, `restore`, and the like; `git -C` on another checkout does not count)
  or wrote a code file of the tree from the shell (`sed -i`, a redirect, a script that writes a
  file it names), no start commit takes every edit, or the failing output names no site outside
  the edited files. A formatter keeps every name and shape, so it does not stop a rebuild.
- The failing output is the result at the failing line, or, when that line makes a Claude tool
  call, the result of that call.
- The truth is each file and line the failing output reports in a file the session did not edit.
  When it reports none, the truth is the file the next edit fixed. A case is caught when a
  `proven` or `likely` row lists a truth file within 2 lines of a truth line (any line for a file).
  A miss takes the cause list of the holdback set; a file-level miss is `fixed file not listed`.
- The marks: at least 8 of 9 of the rebuilt verified cases caught, and at least 70% of the
  rebuilt cases of the other groups.
