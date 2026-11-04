# Guidance audit

Preliminary independent source audit before K3. The lead must verify and
resolve these findings in K4 after the measurement result. No product
behavior change is proposed by this audit.

| Finding | Evidence | K4 disposition |
| --- | --- | --- |
| Skill's "once for each diff" wording omits that only the last checked/blocked hashes are remembered | `edit_check/session.rs` state fields and `hooks.rs` writes; `docs/edit-check.md` documents D1, D2, then D1 blocking again | Pending source verification and wording correction |
| Skill omits the action for `check_edit`'s incomplete-test warning | `formatters.rs` emits broader-command/full-suite fallback; AGENTS and edit-check docs explain it | Pending source verification and instruction correction |
| Possible missing `.env` support wording | `queries.rs` accepts an `env.` pattern family, but pinned Julie 3.11.0 advertises no such emitted pattern or dotenv extension | Rejected: a consumer matcher does not prove extraction. No support claim is added. |

README, edit-check docs, and byte-identical AGENTS/CLAUDE otherwise match the
source on fail-open and last-diff Stop behavior. Existing docs also name
Flask endpoint/view/no-view facts. The preliminary audit ran no tests.

The follow-up `.env` probe returned no config facts for `.env`, `.env.example`,
`.env.local`, or `.env.production`; the latter three were reported unsupported
by Julie, while `.env` had no artifact row. That does not establish why the
exact `.env` path was omitted. The probe used local 2.12.0/3.11.0 executables
without recording their hashes, so it is supporting evidence rather than a
frozen-runtime verification receipt. The fresh pinned capability snapshot
independently contains no `env.*` structural-fact pattern. This uncovered
capability remains outside the plan's extractor scope; existing dotted-key
claims stay limited to TOML, JSON, and YAML.
