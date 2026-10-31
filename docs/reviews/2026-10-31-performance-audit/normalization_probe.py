import argparse
import math
import sqlite3
import time

parser = argparse.ArgumentParser()
parser.add_argument("--db", default="/home/murphy/source/hermes-agent/.code-kb/artifact.db")
parser.add_argument("--repeats", type=int, default=5)
parser.add_argument("--progress-interval", type=int, default=1000)
args = parser.parse_args()

conn = sqlite3.connect(f"file:{args.db}?mode=ro&immutable=1", uri=True)
conn.execute("PRAGMA query_only = ON")

queries = {
    "rust_unlinked": (
        "SELECT symbol_id, path, language, json_extract(metadata_json, '$.impl_type_name') "
        "FROM symbols WHERE parent_symbol_id IS NULL "
        "AND metadata_json LIKE '%\"impl_parent_id_resolved\":false%' "
        "AND json_valid(metadata_json) "
        "AND json_extract(metadata_json, '$.impl_type_name') IS NOT NULL",
        "idx_symbols_unlinked_impl",
    ),
    "go_unlinked": (
        "SELECT symbol_id, path, json_extract(metadata_json, '$.receiver_type') "
        "FROM symbols WHERE parent_symbol_id IS NULL "
        "AND language = 'go' AND kind = 'method' "
        "AND json_valid(metadata_json) "
        "AND json_extract(metadata_json, '$.receiver_type') IS NOT NULL",
        "idx_symbols_unlinked_go_method",
    ),
}

print(f"database={args.db}")
print(f"progress_interval={args.progress_interval} VM instructions per progress callback")
for label, (sql, index) in queries.items():
    variants = {
        "planner": sql,
        "partial_index": sql.replace("FROM symbols ", f"FROM symbols INDEXED BY {index} ", 1),
    }
    for variant, query in variants.items():
        plan = [row[3] for row in conn.execute("EXPLAIN QUERY PLAN " + query)]
        print(f"{label}/{variant} plan={plan}")
        list(conn.execute(query))
        durations_ms = []
        for _ in range(args.repeats):
            started = time.perf_counter_ns()
            rows = sum(1 for _ in conn.execute(query))
            durations_ms.append((time.perf_counter_ns() - started) / 1_000_000)
        p95_ms = sorted(durations_ms)[math.ceil(0.95 * len(durations_ms)) - 1]
        print(f"{label}/{variant} rows={rows} warmup=1 repeats={len(durations_ms)} p95_ms={p95_ms:.3f} samples_ms={[round(x, 3) for x in durations_ms]}")
        counts = []
        for _ in range(args.repeats):
            ticks = [0]

            def progress():
                ticks[0] += 1
                return 0

            conn.set_progress_handler(progress, args.progress_interval)
            rows = sum(1 for _ in conn.execute(query))
            conn.set_progress_handler(None, 0)
            lower = ticks[0] * args.progress_interval
            counts.append((rows, lower, lower + args.progress_interval - 1))
        print(f"{label}/{variant} VM_instruction_bounds={counts}")

conn.close()
