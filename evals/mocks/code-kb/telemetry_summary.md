Telemetry summary (window: {{input.time_window}}, scope: {{input.workspace_only}})
Total calls: 412 (success 396, failed 16, 96.1%)
Est. Tokens Served: ~181,300
Est. Tokens Saved: ~1,240,800 (baseline known for 301 of 412 calls)
Efficiency: 7.8x

| Tool | Calls | Failed | Success | Saved |
|---|---|---|---|---|
| lookup_symbol | 140 | 1 | 99.3% | ~402,100 (120/140) |
| search_symbols | 88 | 0 | 100% | ~310,400 (80/88) |
| get_symbol_context | 61 | 0 | 100% | ~280,900 (61/61) |
| find_references | 52 | 13 | 75.0% | ~120,000 (30/52) |
| codebase_outline | 41 | 0 | 100% | ~0 (0/41) |
| blast_radius | 30 | 2 | 93.3% | ~127,400 (10/30) |

Recent errors:
- find_references: Symbol `Config` not found (x9)
- find_references: database is locked (x4)
- blast_radius: git status failed (x2)
- lookup_symbol: Indexing /work/app started; call again in a few seconds. (x1)
