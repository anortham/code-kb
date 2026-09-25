You are testing the code-kb MCP tools. Work only in the project at PROJECT_ROOT and pass that path as project_root on every code-kb call. Do not edit any file.

Task: use every code-kb tool at least once (codebase_outline, file_skeleton, lookup_symbol, search_symbols, get_symbol_body, get_symbol_context, find_references with direction callers and callees, find_structural_facts, blast_radius, telemetry_summary) to answer these questions:
1. How does Flask turn an incoming WSGI request into a response, and where are errors handled?
2. Which instance attributes does the Flask class set in __init__?
3. Which tests should run after a change to Flask.handle_user_exception, and after a change to src/flask/cli.py?
4. Where is the Flask class imported across the project?
5. Which routes does the tutorial app in examples/tutorial define, with which HTTP methods?

Then write a defect report. For each tool output that was wrong, misleading, incomplete, noisy, or wasteful, give the exact tool call, a short excerpt of the output, and what the correct output should be. Say "no defects" for a tool only when you checked its output against the source. Verify each claim by reading the source files.
