# Ordinal 3 — Flask candidate arm

- Source review: **PASS**.
- Attempt: `target/toolkit-efficiency/campaign/candidate-v2/attempts/002-flask-config-code-kb-r1`.
- Attempt `attempt.json` SHA-256: `4f9e1bf7e1786bdac0dd6311bc1391d28f7b64fdf58b88c9d859e758ba614204` (same as recorded `status.json.outcome_sha256`). Bundle JSON SHA-256: `f1954db0619690ace1aca67ec21b186a9ba66da15552710889d2073aaa60683c`. Seed digest: `fc0b9965e6330ffa0d84577f4d787080eec7ced43c00c8307d6cc6da03d48ead`.
- Reviewed immutable bundle diff against its seed. It changes only `src/flask/app.py`, `src/flask/sessions.py`, and new `tests/test_session_cookie_max_age.py`; bundle violations are empty, `seed_unchanged` is true, and protection is `unchanged`.
- Implementation matches the request: default `None`, permanent-only `timedelta` override, fallback to `PERMANENT_SESSION_LIFETIME`, no config inspection for non-permanent sessions, and named `TypeError` for unsupported permanent values. It preserves the configured permanent lifetime.
- New tests exercise real test-client requests and emitted cookies, override and default expiries, non-permanent cookies with invalid values, absent setting on a non-permanent session, and invalid permanent types. Trusted public replay `evidence/grade/grade-008-stdout_log.log` shows all 16 focused tests passing (the existing test plus all 15 new parameterized cases); hidden replay `grade-010-stdout_log.log` shows its test passing. `grade.json` reports behavioral pass, valid replay, and unchanged protections.
- No acceptance gap found. The final answer's reported full-suite environment failures are outside the retained configured acceptance run; no extra test evidence was used for this source decision.
