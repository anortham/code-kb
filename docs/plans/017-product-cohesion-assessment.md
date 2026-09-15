# 017: code-kb + julie-extract product cohesion assessment (2026-09-15)

Review of code-kb main @ 843eac5 (clean) and julie-extractors main @ d055ec9f (clean).
Evidence: code reading, live `code-kb` and `julie-extract` runs on `~/source/hermes-agent`
(12,788 indexed files, 991k symbols) as the large test tree, fresh scans into the scratchpad,
throwaway two-file repos for defect reproduction, SQLite `dbstat`, server logs.
Status 2026-09-15: findings 1, 2, 3, and the first small item were applied on branch `fix/extractor-seam` (code-kb 1.0.2) and `feat/facts-level` (julie-extract 2.43.0). Findings 4, 5, and 6 are open.

Side effect: the hermes-agent index at `~/source/hermes-agent/.code-kb/artifact.db` was
rebuilt once during measurement. It now records `binary_version = 2.42.3` (it was 2.42.1).
It is stable.

## Verdict

The split is right: julie-extract owns parsing, code-kb owns queries. Warm reads on a
3.9 GB index take under 150 ms. Four gaps sit on the seam between the two products. One is
a live defect, reproduced deterministically.

## Measurements (hermes-agent, 12,788 files, 991k symbols)

| Run | Wall | Peak RSS | DB size |
|---|---|---|---|
| `julie-extract scan` (level full, the default) | 85 s | 5.3 GB | 3.9 GB |
| `julie-extract scan --level symbols` | 42 s | 2.0 GB | 1.3 GB |
| `julie-extract scan --level facts` (2.43.0, what code-kb 1.0.2 requests; keeps type-usage and member-access identifiers) | 52 s | 2.6 GB | 1.7 GB |
| `julie-extract update --file <one .py>` | 0.21 s | 407 MB | |
| `code-kb outline` warm | 0.01 s | 51 MB | |
| `code-kb lookup` warm | 0.13 s | 277 MB | |
| `code-kb refs`, `search`, `blast-radius` warm | 0.00 to 0.01 s | 12 to 22 MB | |
| `code-kb` first call after an extractor upgrade (one rebuild) | 89 s | 5.3 GB | |

Tables code-kb reads: `symbols`, `files`, `relationships`, `pending_relationships`,
`structural_facts`, `literals`, `type_facts`, `artifact_metadata`.

Tables code-kb never reads, with their share of the 3.9 GB (data plus indexes):
`identifiers` ~970 MB, `reference_sites` ~880 MB, `source_regions` ~390 MB, plus
`complexity_metrics`, `type_arguments`, `type_argument_usages`, `symbol_annotations`.
About 60% of the file.

## Findings, ranked

### 1. Defect: version guard rebuilds the index on every call when the extractor is not the pinned build (strong, reproduced)

**Files:** `crates/code-kb-core/src/sync.rs` (`discover_julie_extract_binary`,
`find_julie_extract_binary`, `ensure_index_matches_pin`).

**What happens:** Discovery accepts any `julie-extract` it finds and only warns when its
version differs from the pin. The guard compares the recorded `binary_version` with the pin,
not with the binary in use. When the binary in use is not the pinned build, every read
deletes and rebuilds the index, and the rebuild records the same non-pinned version again.

**Reproduction (two-file repo, extractor 2.42.0 via `JULIE_EXTRACT_BIN`, pin 2.42.3):**

| Step | Recorded version | Rebuilds so far |
|---|---|---|
| `code-kb scan` | 2.42.0 | 0 |
| `code-kb lookup` | 2.42.0 | 1 |
| `code-kb lookup` | 2.42.0 | 2 |
| `code-kb lookup` | 2.42.0 | 3 |
| switch to 2.42.3, `code-kb lookup` | 2.42.3 | 4 |
| `code-kb lookup` | 2.42.3 | 4 |
| switch to 2.42.0, edit a file, `code-kb skeleton` (JIT update) | 2.42.0 | 4 |
| `code-kb lookup` | 2.42.0 | 5 |

A single JIT file update by the non-pinned binary restarts the loop. On hermes-agent one
rebuild costs 89 s and 5.3 GB RSS. The server runs the guard on every start, so every
subagent session pays it.

**How a non-pinned binary gets picked:** discovery walks upward from the current directory
for `.tools/julie-extract` before it checks PATH. A repo that vendors its own extractor
(this is how the defect was first seen) wins over the installed one. Local builds of
julie-extractors with a bumped version hit it the same way.

**Why the tests miss it:** `freshness_test.rs` edits the metadata row by hand. No test runs
the guard with a binary whose version differs from the pin.

**Fix (lazy):**
- Compare the recorded version with the version of the binary code-kb actually found.
  Rebuild once when the binary changes. Keep the pin warning.
- Drop the upward `.tools/` walk from runtime discovery. Keep `JULIE_EXTRACT_BIN`, the
  sibling of the executable, and PATH. The build guard can keep its own `.tools` check.
- One test: recorded version equals the found binary's version but differs from the pin.
  Expect no rebuild.

### 2. Gap: no extraction level matches what code-kb reads (strong)

**Files:** julie `crates/julie-extract-cli` `--level`; code-kb `sync.rs` `scan_workspace`.

**What happens:** `full` writes about 2.3 GB per 3.9 GB that code-kb never reads. `symbols`
drops `structural_facts` and `literals`, which `find_structural_facts` needs. The level
menu was designed around Miller. code-kb always sends `full`.

**Cost today:** 3x disk, 2x scan time, 2.6x peak memory. The worktree fast path copies the
whole file, so each worktree costs another 3.9 GB. The README's "sub-15 MB" claim is true
for the server and false for the product during a scan.

**Fix (lazy):** In julie, make `symbols` keep `structural_facts`, `literals`, and
`type_facts` (or add one level that does). In code-kb, pass that level on `scan`. Expected
on hermes-agent: about 1.4 GB, 45 s, 2 GB RSS.

### 3. Gap: code-kb does not use the extractor's parent watchdog (strong)

**Files:** `sync.rs` `execute_julie_extract`.

**What happens:** julie-extract has `--parent-pid` so a scan aborts when its parent dies.
It was built for a parent like code-kb. code-kb never passes it. A session that dies during
an 85 s, 5 GB scan leaves the scan running. `--spool-dir` and `-j` are also unused.

**Fix:** Pass `--parent-pid <own pid>` on Unix. One line plus a test that the flag is sent.

### 4. Neither product resolves calls (worth exploring)

Julie retired workspace resolution on 2026-08-18 with the note "Miller computes that at
query time". Miller is retired. code-kb matches by name with receiver and namespace hints.
hermes-agent index: 121k resolved vs 520k pending edges.

Of the 519k pending `calls` edges in the hermes-agent index:

| Bucket | Count |
|---|---|
| No internal candidate (stdlib, deps) | 183,378 |
| Exactly one internal candidate | 113,069 |
| Two or more internal candidates | 222,432 |

Two thirds of the internal edges are ambiguous by name. That is where `find_references`
and `blast_radius` merge same-named symbols (`get`, `run`, `handle`). Pending edges carry
`target_receiver` and `target_namespace_json`, and `type_facts` holds resolved types, so
the data for a ranking exists. Do not build a resolver. Prefer, in order: same file, same
namespace, receiver type via `type_facts`. Measure the ambiguous bucket before and after.

### 5. julie-extractors carries Miller-only weight (worth exploring, product decision)

- The `store` command family and versioned family store: 65k lines across the CLI and
  artifact crates. code-kb never calls it.
- JSONL export and five JSONL contract versions. code-kb never calls it.
- `MILLER_ALLOW_EXTRACTOR_DOWNGRADE`, `MILLER_STORE_CHUNK_VERSIONS` env vars.
- 362 files in `crates/*/src` and `docs` mention Miller. The README still says "Miller Ph3
  consumer wiring targets this contract".

If code-kb is the only consumer, this is maintenance debt with no user. The decision is
yours: declare code-kb the sole consumer and delete `store` plus JSONL, or keep them as a
public product surface and say so. The mandate that started code-kb points to delete.

### 6. Two parser stacks for one product (speculative)

code-kb compiles five tree-sitter grammars for `replace_symbol_body` validation. julie
compiles about 40 with a different `tree-sitter` core pin (0.25 vs =0.26.11). A
`julie-extract check --file` command (parse, report error count, exit code) would let
code-kb drop six dependencies and validate every supported language. Cost: one process
spawn per edit, measured at 80 to 210 ms for `update`. Not urgent.

### 7. Small items

- An empty or torn `artifact.db` (for example after a scan killed mid-write) makes plain
  `code-kb scan` fail with `schema_incompatible: no such table: artifact_metadata`, and
  the server has no auto-scan for that case because the file exists. `code-kb scan --force`
  recovers. Fix: on that extractor error, remove the file and rescan, as the version guard
  already does.
- `cargo install code-kb-cli` is listed under "Zero-Dependency" in the README. The build
  guard skips when the pins file is absent, and the first run fails with
  "Extractor binary not found". Add one README line for cargo users and name the release
  page in the error message.
- `.claude-plugin/skills/code-kb/SKILL.md` and `skills/code-kb/SKILL.md` are byte-identical
  copies, like AGENTS.md and CLAUDE.md. `cli_test.rs` already checks the pair.
- Pre-1.0 `telemetry.db` files remain under `<repo>/.code-kb/` in older workspaces.
  Harmless.
- `.code-kb/logs/code-kb.log.2026-09-13` in the code-kb repo is 38 MB from the watcher storm
  loop fixed in 452eebf. Rotation caps it at 7 files. No action.

## Recommended order

1. Finding 1 (defect, one session).
2. Findings 2 and 3 together (one julie release, one code-kb change, one session).
3. Finding 5 decision, then finding 4 with a before/after measurement.
