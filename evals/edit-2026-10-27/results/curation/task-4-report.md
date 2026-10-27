# Task 4 report

## Implemented

- Added the HCL GoHCL regression case under `evals/edit-2026-10-27/cases/hcl/`. The task restores optional `hcl.Expression` fields and a non-nil static null expression when omitted. The reference patch changes `gohcl/schema.go` and `gohcl/decode.go`; the hidden test also checks ordinary scalar fields remain required and pointer fields remain optional.
- Added the date-fns regression case under `evals/edit-2026-10-27/cases/date-fns/`. It restores `options.in` propagation in `addBusinessDays` and `differenceInBusinessDays`. The hidden checks use Singapore and New York calendar boundaries.
- Both cases include source-backed requests, seed/reference patches, hidden file reservations, `expected_output` markers, eligibility data, dependency/runtime setup, protected test/config inputs, generated roots, and two exact production paths.
- Eligibility counts: HCL 123 production files / 24,331 lines; date-fns core/tz/utc 1,286 TypeScript files / 75,623 lines. Neither exact repository identity appeared in the prior evaluation/release scan. HCL retains MPL-2.0; date-fns core retains MIT, with tz/utc licenses protected.

## Source and test evidence

- HCL source confirms `ImpliedBodySchema` makes `hcl.Expression` optional and `decodeBodyToStruct` synthesizes a static null expression for an omitted field. Its public `TestDecodeBody` and `TestImpliedBodySchema` run in the selected command.
- The HCL hidden test checks `ImpliedBodySchema`, then decodes an empty body and evaluates the resulting expression to confirm null. The final sandbox seed reports both the optionality diagnostic and missing-required-expression diagnostic.
- date-fns source confirms `addBusinessDays` supplies `options.in` to `toDate`, and `differenceInBusinessDays` supplies it to `normalizeDates`. The counting test crosses midnight in New York: the correct context returns 1, while the seeded UTC calendar returns 2.
- The first date-fns seed run exposed that the original counting endpoints did not distinguish the missing context. The endpoints were changed, then the final seed failed both hidden tests and the reference passed both.

## Readiness

| Case | Baseline public | Seed hidden | Reference |
| --- | --- | --- | --- |
| HCL | `go test -v ./gohcl`: exit 0; `TestDecodeBody` and `TestImpliedBodySchema` executed | exit 1; expected optional-expression diagnostics | public and `TestEditEvaluationExpressionFieldBehavior`: exit 0 |
| date-fns | Vitest main project: 2 files passed; 34 tests passed, 1 skipped | exit 1; both tests fail, including `expected 2 to be 1` | public regressions pass; both hidden tests pass |

- Final HCL logs: `target/edit-evaluation/readiness/task4-readiness-20261027T143138Z/`.
- Date-fns baseline logs: `target/edit-evaluation/readiness/task4-readiness-20261027T144211Z/`; final seeded/reference logs after the count-case correction: `target/edit-evaluation/readiness/task4-readiness-20261027T144704Z/`.
- `expected_output` markers were checked against retained baseline/reference logs, including actual test names and summary counts. Seed diagnostics were checked against the declared failure fragments.
- Runtime setup copies canonical caches into fresh `/state`; package installs and tests ran only through the reviewed sandbox. Date-fns uses `pnpm install --offline --trust-lockfile --frozen-lockfile --ignore-scripts`. The initial offline attempt tried registry policy checks despite `--offline`; it was stopped only in its own sandbox process group, and its partial logs remain. The final offline installs passed. The `--trust-lockfile` option skips registry verification of lockfile supply-chain policies per the [pnpm install documentation](https://pnpm.io/cli/install).
- Canonical input host maps, hashes, tool versions, and run paths are recorded in ignored `target/edit-evaluation/readiness/runtime-bindings.json`.
- Earlier readiness failures remain retained under `target/edit-evaluation/readiness/`: a dangling `/usr/bin/go` symlink, a compile-only HCL seed caused by an unused import, the first date-fns offline install, and the initial count case. Each was corrected and the final readiness runs passed.

## Verification and state

- Seed then reference patches apply in order and restore the pinned production files. Both source clones remain clean at HCL `4a347c448ee961e556dde0fad522a996e04c00c0` and date-fns `a9739fb3e5384a810e90d7376446f6d84ecc5029`.
- The MCP `check_edit` call could not initialize its file watcher; the local CLI fallback `target/release/code-kb --root /home/murphy/.codex/worktrees/1e6f/code-kb check` reported no symbol shape changed against `7d7073a8`.
- No upstream install, build, or test command ran on the host. No files were staged or committed by this task.
- Task worktree: `/home/murphy/.codex/worktrees/1e6f/code-kb`, branch `fix/cross-language-review`, HEAD `7d7073a8b73a91fb98bf98d002d2247c5fc815b0`; `git status --short --branch` shows `?? evals/edit-2026-10-27/cases/`, `?? scripts/edit_evaluation.py`, and `?? scripts/test_edit_evaluation.py`. The evaluator scripts are parallel-lead-owned.
- Main worktree: `/home/murphy/source/code-kb`, branch `main`, HEAD `aeebe72a21c9057a0b63d75b5cd9a6db3884893d`, clean and two commits ahead of `origin/main`. `git worktree list` shows only the main and task worktrees.
