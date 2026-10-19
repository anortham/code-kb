# Edit replay cases

`scripts/edit_replay.py` replays `code-kb check` and the Stop hook on real commits, with no model
run. It measures two things:

- How often the Stop hook blocks a diff that needs no more edits (a false block).
- How many of the sites that an edit must reach the check lists.

The harness calls only the `code-kb` CLI and git. It works in full-history clones under
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
  callable with that name has other parameter text. A type counts only when the diff removes it.
  A definition whose head spans lines is seen only when its first line changes.
- The truth is the removed lines of other files in the same language family (JavaScript and
  TypeScript are one family) that use the name and whose use the commit changed: the name with
  its argument text (or the bare name) is on fewer added lines than removed lines of that file.
  A line that changed for another reason keeps its use and is not truth. Comment lines are not
  truth. Truth lines carry their line numbers in the parent.
- A case is the commit, the defining file, and the name. A file renamed by the commit is skipped.
  A name gets at most `--per-symbol` cases per project, and a commit at most `--per-commit`, so one
  large refactor cannot fill the set.
- `run` checks out the parent, applies the commit's change to the defining file only, and runs
  `code-kb check --base <parent>`. A truth site is caught when a `proven` or `likely` row of a
  change of that name in the defining file has its path and line. Each miss gets one cause:
  `shape not seen` (the check reports no change of that name), `cut by a limit`, or, from
  `code-kb refs` of the name on the parent tree, `site not indexed`, `site labeled unresolved`,
  `site excluded`, or `other`.
- `score` prints per language: truth sites, the ones caught, the misses, the unresolved counts,
  cases with no shape change found, and the listed rows outside the truth. A row outside the truth
  is true when its site refers to the changed definition (not another one with that name).
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
  the session start on its branch or on any branch where every edit applies. The repository is a
  `git clone --shared` of the session's main repository, so the owner's checkouts never change.
- A case is not rebuilt, with its cause, when the log or the repository is gone, a command before
  the failing line changed the git state (`checkout`, `reset`, `stash`, `rebase`, `merge`, `pull`,
  `restore`, and the like) or edited files from the shell (`sed -i`, `cat >`, a formatter), no
  start commit takes every edit, or the failing output names no site outside the edited files.
- The truth is each file and line the failing output reports in a file the session did not edit.
  When it reports none, the truth is the file the next edit fixed. A case is caught when a
  `proven` or `likely` row lists a truth file within 2 lines of a truth line (any line for a file).
- The marks: at least 8 of 9 of the rebuilt verified cases caught, and at least 70% of the
  rebuilt cases of the other groups.
