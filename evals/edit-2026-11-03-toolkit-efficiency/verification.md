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

Branch gate and confirmation evidence are pending. Security scope: none
declared by the plan. Reviewer choice: none. The lead reviews worker output
and completed patches against the frozen acceptance criteria.
