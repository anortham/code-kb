# Reach gate

An offline gate for `find_references` callers answers. It runs a code-kb binary over frozen,
adjudicated cases in 7 languages and scores each label tier against ground truth. The gate needs
only code-kb and the case files. SCIP indexes are used once, to build the cases.

## Files

| Path | Purpose |
|---|---|
| `corpus.json` | The 7 pinned projects: name, language, URL, full commit, optional local clone. |
| `cases/<repo>.json` | 30 frozen symbols per project with truth and adjudications. |
| `../../scripts/reach_gate.py` | `materialize`, `run`, `disagreements`. |
| `../../scripts/reach_truth.py` | `sample` and `truth`, from a SCIP index. |
| `../../scripts/scip_decode.py` | Standard-library SCIP reader used by `reach_truth.py`. |
| `../../scripts/test_reach_gate.py` | Unit tests. They build their own fixtures and use no network. |

## Corpus

| Name | Language | Commit |
|---|---|---|
| flask | python | `d73fa1cdcbd8b1465c151db8924ba58b1dd14e35` |
| express | javascript | `bed501c695a61886399ee622875f3be933c716d8` |
| zod | typescript | `e516c3baf22615e20934116abebfed6c000222c2` |
| gson | java | `8b4b55051489132190cb8d1c61eb9dc7f5381295` |
| cobra | go | `adbc8813901bba65827259daa8e22ff94ec1f30e` |
| newtonsoft-json | csharp | `09bb545d72969ad7fb4ea07db0d5c34f4fc07877` |
| code-kb | rust | `765d94079677cd21fc180f9b3e1e266b02c83a31` |

`reach_gate.py materialize` clones each project into `~/.cache/code-kb-reach/<name>-<first 12 of commit>`
(from `local` when that folder exists, else from `url`) and checks out the commit. A second run
reuses the copies. Nothing writes into a copy except the `.code-kb/` index, which ignores itself.

## Case file

```json
{
  "repo": "flask",
  "commit": "d73fa1cdcbd8b1465c151db8924ba58b1dd14e35",
  "language": "python",
  "symbols": [
    {
      "id": "from_mapping@src/flask/config.py:304",
      "split": "dev",
      "name": "from_mapping",
      "file": "src/flask/config.py",
      "line": 304,
      "kind": "method",
      "tags": ["value-ref"],
      "truth_source": "scip",
      "truth": [{"path": "src/flask/config.py", "line": 302}],
      "adjudicated": [{"path": "docs/x.rst", "line": 4, "verdict": "false", "note": "a prose mention"}]
    }
  ]
}
```

- An optional top-level `scope` (a list of path prefixes, such as `["Src/Newtonsoft.Json/"]`) limits scoring to sites and text lines under those prefixes. Use it when the truth covers only part of the repo.
- `name` is the bare name, not `Owner.name`. `file` and every `path` use `/` and are relative to the project root.
- `split` is `dev` or `holdout`. Tune fixes on dev only.
- `truth_source` is `scip`, `manual`, or `scip+manual`. `reach_truth.py truth` rewrites `truth` for
  `scip` and `scip+manual` symbols and never touches `adjudicated` or `manual` symbols.
- Effective truth is `truth`, minus lines adjudicated `false`, plus lines adjudicated `true`.
  Write a `true` note that names the call or value use. Write a `false` note that says why the line does not reach the target.
- `tags` come from `sample`: `common-name` (3 or more definitions with the name), `overridden`,
  `many-refs` (20 or more reference lines), `value-ref` (a reference not followed by `(`), `decorator` (Python `@name`).

## Commands

```
scripts/reach_gate.py materialize [--repos a,b]
scripts/reach_gate.py run [--binary PATH] [--repos a,b] [--out DIR] [--show-holdout]
scripts/reach_gate.py disagreements --repo R [--split dev|holdout|all] [--binary PATH] [--show-holdout]
scripts/reach_truth.py sample --repo R --index INDEX.scip --count 30 --seed 20261017 [--path-prefix P] [--out FILE]
scripts/reach_truth.py truth  --repo R --index INDEX.scip [--path-prefix P] [--write]
scripts/reach_truth.py truth  --repo R --index INDEX.scip --symbol Config.from_mapping
```

`--binary` defaults to `target/release/code-kb`, else `code-kb` on `PATH`. `--out` defaults to `target/reach-gate/`.
Both scripts also take `--corpus`, `--cache-dir`, and (gate and truth) `--cases-dir`, for tests and scratch work.

For each case symbol the gate runs `code-kb --root <copy> scan` once per project, then
`--json lookup <name> --path <file> --limit 50` to find the symbol id (the row whose start line equals
the case line, else the nearest row with that name in that file), then `--json refs --symbol-id <id> --limit 200`.

The gate reads two answer shapes. A v2.6.0 array of `ReferenceSite` rows scores every row as `proven`, with no text lines.
The candidate report object (`sites[].label`, `sites[].basis`, `sites[].end_line`, `text_only`) scores each label.

## Scoring

A truth line `(P, L)` matches a site when `site.path == P` and `site.start_line <= L <= (site.end_line or site.start_line)`.

- Proven precision: proven sites that cover a truth line, divided by all proven sites.
- Recall: truth lines covered by a proven or likely site, divided by truth lines.
- Accounted share: truth lines covered by any site, or listed in `text_only`, divided by truth lines.
- Per label tier (proven, likely, unresolved, excluded) the report lists sites that cover a truth line over all sites.

Pass marks, per language over dev and holdout together: proven precision at least 0.98 and accounted share at least 0.98.
The recall target of 0.90 is reported but does not decide the result.
A language with no proven sites or no truth lines fails. A project with no case file counts as not scored and fails the run,
so a partial run (`--repos`) never ends in PASS.

`run` writes `report.json` and `report.md` and exits 0 on PASS, 1 on FAIL (the report is still written), 2 on a setup error.
`report.md` carries a `Commit:` line (the HEAD of this checkout) and ends with `## Result: PASS` or `## Result: FAIL`.
Holdout symbols are not listed in the report or in `disagreements` output unless `--show-holdout` is set; their totals always show.

`scripts/release-preflight.sh` (step 10) requires `target/reach-gate/report.md` with `## Result: PASS` and a `Commit:` line that names an ancestor of HEAD.

## Building truth

1. `reach_gate.py materialize`.
2. Build a SCIP index of the project copy (below).
3. `reach_truth.py sample --repo R --index X.scip --out evals/reach/cases/R.json`.
4. `reach_truth.py truth --repo R --index X.scip --write`.
5. `reach_gate.py run --binary /home/murphy/.local/bin/code-kb --repos R --out target/reach-gate/baseline-R`.
6. `reach_gate.py disagreements --repo R --split all --show-holdout`, then adjudicate every line in the case file.

Truth is every reference occurrence of the symbol minus definitions, and minus import, `use`, and `using` lines.
SCIP also lists some lines that belong to a definition (a Python property setter, an `@overload` stub, a TypeScript object-literal method that implements an interface member); the case files adjudicate them false.
Overloads with the same owner and name are unioned. Cases skip test files and generated or build folders.
A name with more than 250 reference lines is not sampled.

| Project | Indexer | Command (run in the project copy) |
|---|---|---|
| flask | scip-python 0.6.6 | `python3 -m venv V && V/bin/pip install werkzeug jinja2 itsdangerous click blinker markupsafe pytest asgiref python-dotenv && V/bin/pip install -e . --no-deps`, then `PATH=V/bin:$PATH scip-python index --project-name flask --project-version d73fa1c --output X.scip`. Install flask editable, or tests resolve to the wrong module. |
| express | scip-typescript 0.4.0 | `npm install --no-package-lock --ignore-scripts`, then `scip-typescript index --infer-tsconfig --no-progress-bar --output X.scip`. It writes `tsconfig.json` into the copy; delete it afterwards. Methods assigned as properties (`app.use`, `res.send`) are local symbols, so their truth is `manual`. |
| zod | scip-typescript 0.4.0 | In a separate copy: `pnpm install --frozen-lockfile --ignore-scripts`, then `scip-typescript index --no-progress-bar --output X.scip` with the project's own `tsconfig.json`. `packages/bench` has an npm alias of zod 4.0.8 whose symbols merge with the workspace symbols; the case file adjudicates those lines false. `packages/docs` is outside the tsconfig, so it has no truth. |
| gson | scip-java 0.12.3 | JDK 17, Maven 3.9: `cs launch com.sourcegraph:scip-java_2.13:0.12.3 -- index --output X.scip`. JDK 21 and 25 fail. |
| cobra | scip-go 0.2.7 | `scip-go index --repository-remote github.com/spf13/cobra --module-version adbc881 --output X.scip`. Install from `github.com/scip-code/scip-go/cmd/scip-go`. |
| newtonsoft-json | scip-dotnet 0.2.14 | In `Src/Newtonsoft.Json`: `LibraryFrameworks=net8.0 scip-dotnet index Newtonsoft.Json.csproj --output X.scip`, then pass `--path-prefix Src/Newtonsoft.Json/` to `reach_truth.py`. Scope symbols and truth to the library; the test project does not resolve. The SDK pin in `Src/global.json` may need a lower version. |
| code-kb | rust-analyzer 1.97 | `rust-analyzer scip . --output X.scip`. Host-inactive `cfg` blocks are not indexed. |

Put heavy output (copies, indexes, virtual environments) under `~/.cache/code-kb-reach/` or `~/.cache/code-kb-accuracy/`, not `/tmp`.
