"""Write a fixed code-kb telemetry database into the eval run's temporary home.

Usage: seed.py <plugin.json>. The rows carry the plugin's version, so `code-kb stats`
counts them under its default version filter. The current folder is the project.
"""

import json
import os
import sqlite3
import sys
import uuid

PROJECT_TOOLS = [
    # tool, calls, failures, tokens saved, calls with a known baseline
    ("lookup_symbol", 140, 1, 402_100, 120),
    ("file_skeleton", 88, 0, 310_400, 80),
    ("get_symbol_context", 61, 0, 280_900, 61),
    ("find_references", 52, 13, 120_000, 30),
    ("get_symbol_body", 41, 0, 0, 0),
    ("blast_radius", 30, 2, 127_400, 10),
]
ERRORS = {
    "lookup_symbol": ["Indexing /work/app started; call again in a few seconds."],
    "find_references": ["Symbol `Config` not found"] * 9 + ["database is locked"] * 4,
    "blast_radius": ["git status failed"] * 2,
}
OTHER_PROJECT_TOOLS = [("lookup_symbol", 70, 0, 90_000, 70), ("find_references", 30, 0, 40_000, 30)]


def rows(root, name, tools, version, errors, first_hour, hours):
    hour = 0
    for tool, calls, failures, saved, known in tools:
        messages = list(errors.get(tool, []))
        assert len(messages) == failures, tool
        for call in range(calls):
            hour = (hour + 7) % hours
            failed = call < failures
            has_baseline = not failed and call - failures < known
            share = saved // known + (saved % known if call == failures else 0) if has_baseline else 0
            yield (
                str(uuid.uuid4()), f"-{first_hour + hour} hours", root, name, tool, 40,
                "error" if failed else "ok", messages[call] if failed else None,
                0 if failed else 5, 0 if failed else 1, 1_800, 440,
                share, 1 if has_baseline else 0, version,
            )


def main():
    version = json.load(open(sys.argv[1]))["version"]
    home = os.environ["HOME"]
    folder = os.path.join(home, ".code-kb")
    db = os.path.join(folder, "telemetry.db")
    if os.path.exists(db):
        sys.exit(f"refusing to overwrite an existing telemetry database: {db}")
    os.makedirs(folder, exist_ok=True)
    os.makedirs(".git", exist_ok=True)
    root = os.path.realpath(os.getcwd())
    conn = sqlite3.connect(db)
    conn.execute(
        "CREATE TABLE tool_telemetry (id TEXT PRIMARY KEY, timestamp TEXT NOT NULL,"
        " workspace_root TEXT NOT NULL, workspace_name TEXT NOT NULL, tool TEXT NOT NULL,"
        " duration_ms INTEGER NOT NULL, outcome TEXT NOT NULL, error_message TEXT,"
        " result_count INTEGER NOT NULL DEFAULT 0, result_count_known INTEGER NOT NULL DEFAULT 0,"
        " bytes_returned INTEGER NOT NULL DEFAULT 0, est_tokens INTEGER NOT NULL DEFAULT 0,"
        " est_tokens_saved INTEGER NOT NULL DEFAULT 0,"
        " est_tokens_saved_known INTEGER NOT NULL DEFAULT 0, code_kb_version TEXT NOT NULL,"
        " reconcile_ms INTEGER DEFAULT NULL, query_ms INTEGER DEFAULT NULL)"
    )
    insert = (
        "INSERT INTO tool_telemetry (id, timestamp, workspace_root, workspace_name, tool,"
        " duration_ms, outcome, error_message, result_count, result_count_known, bytes_returned,"
        " est_tokens, est_tokens_saved, est_tokens_saved_known, code_kb_version)"
        " VALUES (?1, datetime('now', ?2), ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
    )
    conn.executemany(insert, rows(root, os.path.basename(root), PROJECT_TOOLS, version, ERRORS, 0, 150))
    conn.executemany(insert, rows("/work/other", "other", OTHER_PROJECT_TOOLS, version, {}, 200, 300))
    conn.commit()
    conn.close()


if __name__ == "__main__":
    main()
