#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
JULIE="${JULIE_EXTRACT_BIN:-${REPO_ROOT}/.tools/julie-extract}"
TMP_ROOT="$(mktemp -d)"
trap 'rm -rf "${TMP_ROOT}"' EXIT

"${JULIE}" scan --root "${SCRIPT_DIR}/fixture" --db "${TMP_ROOT}/artifact.db" --level references --json >"${TMP_ROOT}/scan.json"
python3 - "${TMP_ROOT}/artifact.db" <<'PY'
import sqlite3
import sys

connection = sqlite3.connect(sys.argv[1])
symbols = {
    (name, path)
    for name, path in connection.execute(
        "SELECT name, path FROM symbols WHERE name IN (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        ("register_account", "registerAccount", "createApp", "send", "submitInvitation", "postInvitation", "Send", "ServeHTTP", "InvitationController", "handle_invitation"),
    )
}
required = {
    ("register_account", "python/app.py"),
    ("registerAccount", "javascript/registration.cjs"),
    ("createApp", "javascript/app.cjs"),
    ("submitInvitation", "typescript/controllers/invitation.ts"),
    ("postInvitation", "typescript/routes.ts"),
    ("ServeHTTP", "go/invitation.go"),
    ("send", "java/src/main/java/review/InvitationService.java"),
    ("Send", "csharp/InvitationService.cs"),
    ("handle_invitation", "rust/src/lib.rs"),
}
missing = sorted(required - symbols)
if missing:
    raise SystemExit("missing extracted symbols: " + ", ".join(f"{name} in {path}" for name, path in missing))
calls = {
    (path, target)
    for path, target in connection.execute(
        "SELECT path, target_terminal_name FROM pending_relationships"
    )
}
required_calls = {
    ("csharp/InvitationController.cs", "Send"),
    ("go/invitation.go", "Send"),
    ("java/src/main/java/review/InvitationController.java", "send"),
    ("rust/src/lib.rs", "send"),
    ("typescript/controllers/invitation.ts", "send"),
    ("typescript/routes.ts", "submitInvitation"),
    ("typescript/app.ts", "postInvitation"),
}
missing_calls = sorted(required_calls - calls)
if missing_calls:
    raise SystemExit("missing extracted calls: " + ", ".join(f"{name} in {path}" for path, name in missing_calls))
test_calls = {
    (path, name)
    for path, name in connection.execute("SELECT path, name FROM identifiers WHERE kind = 'call'")
}
required_test_calls = {
    ("go/invitation_test.go", "ServeHTTP"),
    ("javascript/registration.test.cjs", "createApp"),
    ("java/src/test/java/review/InvitationControllerTest.java", "post"),
    ("csharp/InvitationControllerTests.cs", "Invite"),
    ("rust/tests/invitation.rs", "handle_invitation"),
}
missing_test_calls = sorted(required_test_calls - test_calls)
if missing_test_calls:
    raise SystemExit("missing extracted test calls: " + ", ".join(f"{name} in {path}" for path, name in missing_test_calls))
bindings = set(
    connection.execute(
        "SELECT path, name FROM identifiers WHERE kind = 'variable_ref' AND path = 'javascript/app.cjs'"
    )
)
if ("javascript/app.cjs", "registerAccount") not in bindings:
    raise SystemExit("missing CommonJS handler binding in javascript/app.cjs")
framework_edges = {
    (path, target)
    for path, target in connection.execute(
        "SELECT path, target_terminal_name FROM pending_relationships"
    )
}
missing_framework_edges = {
    ("python/test_app.py", "register_account"),
    ("javascript/registration.test.cjs", "registerAccount"),
} & framework_edges
if missing_framework_edges:
    raise SystemExit("framework test-edge gaps changed: " + ", ".join(f"{name} in {path}" for path, name in sorted(missing_framework_edges)))
print(f"extracted {len(required)} expected symbols, {len(required_calls)} static calls, {len(required_test_calls)} test calls, and the CommonJS value binding")
print("confirmed missing Python and CommonJS framework test edges for local gap tracking")
PY
