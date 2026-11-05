# Guidance audit

Independent source audit before K3, verified and resolved by the lead after
the final negative measurement result. Only the two skill wording corrections
below are retained; product behavior stays unchanged.

| Finding | Evidence | K4 disposition |
| --- | --- | --- |
| Skill's "once for each diff" wording omits that only the last checked/blocked hashes are remembered | `SessionState::has_seen` in `edit_check/session.rs`, `stop_decision` in CLI `hooks.rs`, and `docs/edit-check.md` | Corrected: two hashes per session, silent repeats, possible D1/D2/D1 block, and fail-open errors |
| Skill omits the action for `check_edit`'s incomplete-test warning | `push_tests_incomplete` in `formatters.rs:3112` emits broader-command/full-suite fallback; AGENTS and edit-check docs explain it | Corrected: run the emitted broader command or the relevant full suite with its configured runner |
| Possible missing `.env` support wording | `queries.rs` accepts an `env.` pattern family, but pinned Julie 3.11.0 advertises no such emitted pattern or dotenv extension | Rejected: a consumer matcher does not prove extraction. No support claim is added. |

README, edit-check docs, and byte-identical AGENTS/CLAUDE otherwise match the
source on fail-open and last-diff Stop behavior. Existing docs also name
Flask endpoint/view/no-view facts. They remain unchanged.

The guidance changes were made after all 36 timed trials and were not measured
by that campaign. They make no savings claim. The rejected initialize candidate
and its exact handshake assertion were restored byte-for-byte to baseline.
Existing `edit_hook_test` cases cover silent repeats and changed diffs;
`edit_check_test` covers incomplete lists and broader commands. No new behavior
requires a new regression test. The final workspace and plugin checks are
recorded in `verification.md`; no separate skill pressure campaign was run.

The follow-up `.env` probe returned no config facts for `.env`, `.env.example`,
`.env.local`, or `.env.production`; the latter three were reported unsupported
by Julie, while `.env` had no artifact row. That does not establish why the
exact `.env` path was omitted. The probe used local 2.12.0/3.11.0 executables
without recording their hashes, so it is supporting evidence rather than a
frozen-runtime verification receipt. The fresh pinned capability snapshot
independently contains no `env.*` structural-fact pattern. This uncovered
capability remains outside the plan's extractor scope; existing dotted-key
claims stay limited to TOML, JSON, and YAML.
