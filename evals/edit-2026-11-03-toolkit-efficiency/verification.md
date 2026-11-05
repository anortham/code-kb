# Verification ledger

Worktree: `/home/murphy/.codex/worktrees/dd7b/code-kb`, branch
`toolkit-efficiency`. Initial source commit:
`f290fdca44cec942ae6f50265e21da9f37fca836`. The main checkout was clean,
one planning commit ahead of origin. No push or release is authorized.

| UTC date | Invariant | Command or evidence | Scope | Source | Result |
| --- | --- | --- | --- | --- | --- |
| 2026-11-04 | Exact pinned extractor | `bash scripts/restore-julie-extract.sh` | Linux x86_64, Julie 3.11.0 | f290fdc | Download checksum verified; restored |
| 2026-11-04 | Build current runtime locally | `cargo build --release` | Workspace release binary | f290fdc | Passed; identity in runtime-current.json |
| 2026-11-04 | Runner baseline | `python3 -B -m unittest discover -s scripts -p 'test_completed_edit_campaign.py'` | Campaign runner | f290fdc | 22 tests passed |
| 2026-11-04 | Frozen Codex command isolation | Existing `test_agent_profile_selftest_needs_no_private_profile_or_auth` and `test_agent_profile_denies_private_reads_writes_and_command_network`, with `CODEX_NATIVE` bound to the frozen 0.164.0 executable | No-provider sandbox probes | f290fdc | 2 tests passed; private profile reads/writes and command network denied |
| 2026-11-04 | Current profile provenance | Compare frozen code-kb guidance with current routing block, two newlines, and skill body without front matter | Guidance/profile inputs | f290fdc | Byte-identical; 7,924 guidance characters, 8,590 profile instruction characters; these are not token measurements |

K1 passes the frozen-manifest validator (6 cases, 24 primary slots) and the existing runner with an empty schedule selection (0 model dispatches). The frozen combined schedule has 36 jobs and 12 matched sets. Manifest SHA-256: `650a038b104c036c850fd05a798cf68c42f5836c3c0b57ce89742e4b6cf5ba81`. All six accepted readiness derivatives have lead source review and match final case/source/seed identities. See `control-current.json` and `readiness/source-review.md`.

Confirmation and the branch gate are complete. Security scope: none declared
by the plan. Reviewer choice: none. The lead reviewed worker output and every
completed patch against the frozen acceptance criteria.

## K2 candidate

Source `3dc4d5d` passed the expected red/green handshake test, all 66 MCP tests, formatting, and the release build. Real baseline/candidate handshakes have equal tool definitions and initialize results except the 480-to-158-character shared instructions. Candidate runtime, source, binary hash, controls, and zero-dispatch compatibility are bound in `runtime-candidate.json`, `schema-comparison.json`, and `control-candidate.json`. K3 rejected this candidate; its source and executable remain preserved as evidence.

## K3 setup repair

Ordinal 1 native completed with accepted source and 19 added tests passing on the trusted fresh public replay (20 public checks total). Ordinal 2 current stopped before exposure: the framework cases lacked the managed `.code-kb` root. Versioned metadata-only corrections preserve prompts, source, checks, protected inputs and seeds; all six real preindex paths pass. Original records are unchanged. Host `/tmp` quota was relieved through the specifically approved, hash-verified cache relocation. See `setup-v2/protocol-amendment.md` and `results/infrastructure.json`. The unchanged schedule resumed only the unexposed slot and completed all 36 model trials once.

## K3 final result and restoration

Every arm accepted 12/12 patches and 2/2 for every task, with no protection
violation. All required measurements are present. Actual added Java/C# classes
ran in 12 separate fresh public-only replays: 41 Java and 190 C# tests passed,
with no failures or skips. Canonical public and hidden checks, independent
source review, replay selectors, hashes and all 12 matched sets are in
`results/report.md`, `results/metrics.json`, and `results/source-review/`.

Candidate/current input rose 14.47%, uncached input fell 6.55%, and
index-inclusive time rose 10.35%. Both frozen efficiency gates fail. The
candidate is rejected with no savings claim. Its two product/test files were
restored byte-for-byte to `f290fdc`: the restored expectation first failed
against the trial instructions (exit 101), then passed after restoring the
server (exit 0). Logs are in `method/k3/restore-{red,green}.log`.

An independent read-only audit recomputed every raw total, acceptance binding,
native N/A index value and gate. Additional matrix, completion and known-empty
protection guards reproduce `metrics.json` byte-for-byte; see
`results/analysis-verification.json`. The interim summary before three Java
acceptance imports finished remains separate and inconclusive. No raw trial,
case, grader, threshold or measurement was changed.

## K4 and final branch gate

Only skill wording changes remain: incomplete test lists require the emitted
broader command or the full relevant configured suite; Stop hooks remember
their last checked and blocked hashes, repeat silently, and fail open on errors.
`SessionState::has_seen`, `stop_decision`, `push_tests_incomplete`, existing hook
and edit-check tests, and `docs/edit-check.md` establish these rules. The changes
were made after timing and are unmeasured. No new behavior required a new test
or another skill pressure campaign. The audit is in `documentation-audit.md`.

The gate ran in the same worktree at source HEAD
`98644c4aff7bd5489b59d279cb611471f573b91a` plus the final reviewed changes.
Runtime code is byte-identical to baseline `f290fdc`; the local release binary
also reproduces baseline SHA-256
`d3f913db2025339a3496f85017eeaee98542f78afa2f654aacbdf2cc108221d2`.

| UTC date | Command | Scope | Result |
| --- | --- | --- | --- |
| 2026-11-04 | `cargo fmt --all -- --check` | Workspace source | Passed |
| 2026-11-04 | `cargo clippy --workspace --all-targets -- -D warnings` | All workspace targets | Passed |
| 2026-11-04 | `cargo test --workspace --locked` | Full Rust workspace and doctests | 2,418 passed; 0 failed or ignored |
| 2026-11-04 | `python3 -m unittest discover -s scripts -p 'test_*.py'` | Full script suite | 305 passed |
| 2026-11-04 | `node --test tests/plugin/*.test.cjs` | Plugin suite | 20 passed; 0 skipped |
| 2026-11-04 | `cmp AGENTS.md CLAUDE.md` | Guidance mirrors | Passed; neither changed |
| 2026-11-04 | `cargo build --release` | Restored local runtime | Passed; baseline hash reproduced |
| 2026-11-04 | `check_edit(base="98644c4")` | Final product/skill edits | No symbol changed shape |

Exact commands, observed timestamps, log hashes and source state are in
`results/branch-gate.json`. Raw logs are preserved under `method/branch-gate/`.
The historical evaluation trees and runtime sources remain unchanged against
`f290fdc`. Patch/log data retain their original bytes; whitespace checks exclude
those evidence formats. Archives and verified member inventories are bound in
`results/archive.json`. Linux was verified; no new Windows run was needed for
unchanged runtime code. The branch stays local, with no push, release or global
installation.
