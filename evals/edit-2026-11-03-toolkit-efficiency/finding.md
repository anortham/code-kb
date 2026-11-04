# K1 trace finding

Selected candidate: shorten only the MCP initialize `instructions` paragraph
in `McpServer::handle_request`, in
`crates/code-kb-cli/src/mcp/server.rs`. Preserve every individual tool
description, schema, handler, output, warning, and the routing/skill guidance.
The regression seam is the initialize assertion in
`test_mcp_blast_radius_stdio_handshake_and_tools`, in
`crates/code-kb-cli/tests/mcp_test.rs`. K2 owns those two files only.

The current server sends a 480-character shared instruction paragraph. The
connected client appends that paragraph to each of the seven tool
descriptions. The frozen profile also injects the routing block and skill,
which carry the same navigation and edit rules. All twelve one-call trials
used MCP. Applying the observed client behavior to twelve trials projects
84 description occurrences; the historical stdout traces do not capture the
provider's tool-schema inputs, so 84 is not an observed provider-input count.
The current live metadata proves repetition in seven descriptions. Character
and occurrence counts identify a candidate; they are not measured input-token
savings. Tool descriptions and schema fields already explain each tool and
require the project root. The campaign pins CLI 0.164.0; its measured effect,
including any difference in client presentation, must be established by K3.

Replace the paragraph with:

> Pass project_root, the absolute path of the project or git worktree you work
> in, on every call. Follow each tool's description for navigation and edit checks.

The candidate keeps the root instruction and points to the tool-specific
rules. Actual token, quality, and index-inclusive time effects are unknown
until K3. A candidate that fails its frozen retention gate is not retained.

## Trace evidence

The immutable historical sources are:

- `evals/edit-2026-10-27/results/trace-review/mcp-traces.json` and
  `results/adjudication/final-cohort-metrics.json` in that study.
- `evals/edit-2026-10-31-one-call/results/metrics.json`, its `report.md`,
  `protocol-v3.md`, and frozen code-kb profile.
- Redacted raw `evidence/agent/stdout.log` files below
  `/home/murphy/.cache/code-kb-completed-edit/2026-10-31-one-call/campaign/one-call-79a6abda/attempts/`.
  The historical `results/artifact-map.json` binds their archive and hashes.
- Current `server.rs` initialize instructions and connected-client tool
  description metadata, together with the source-matched 7,924-character
  routing/skill guidance and 8,590-character profile instructions.

The older completed cohort has 247 code-kb calls and 12/12 accepted patches.
The one-call cohort has 134 calls, including 40 context calls, and 12/12
accepted patches. Its total input rose 0.7%, uncached input fell 18.7%, and
output fell 4.9%. Eleven timed trials improved index-inclusive time about
11%; slot 15 timing is unavailable. The engine, extractor, and guidance all
changed without a fresh native arm, so these differences do not isolate a
cause or prove current task-wide savings.

Raw one-call traces contain no repeated identical navigation request within
a trial. Repeated identical `check_edit` arguments occur in six trials:
slots 2, 12, 13, 15, and 23 call it twice, and slot 10 three times, after
their working-tree changes. Context followed by an explicit same-file shell
read occurs in 10/12 trials, as required by the edit protocol. The earlier
cohort repeats a context request only twice, both in slot 13. Removing these
calls or reads is not justified. Truncating answers would risk required
callers, tests, conventions, uncertainty, and drill-down paths. Neither is
the selected candidate.

The initial aggregate-only review suggested no candidate. Raw trace review
and current client instruction metadata identified the narrower repetition
above. Selection precedes production edits and confirmation exposure. Case
readiness and the complete manifest freeze must finish before K2 begins.
