#!/bin/sh
# code-kb — POSIX shell SessionStart and SubagentStart activation hook

EVENT="SessionStart"
for arg in "$@"; do
    if [ "$arg" = "SessionStart" ] || [ "$arg" = "SubagentStart" ]; then
        EVENT="$arg"
    fi
done

# Try native code-kb binary first if available
if command -v code-kb >/dev/null 2>&1; then
    exec code-kb hook "$EVENT"
fi

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
MD_FILE="${SCRIPT_DIR}/code-kb-routing-block.md"

if [ -f "$MD_FILE" ]; then
    CONTENT="$(cat "$MD_FILE")"
else
    CONTENT="## Code Intelligence: Always use code-kb MCP tools"
fi

# Escape JSON string safely
ESCAPED_CONTENT=$(printf '%s' "$CONTENT" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' | awk '{printf "%s\\n", $0}' | sed 's/\\n$//')

printf '{"hookSpecificOutput":{"hookEventName":"%s","additionalContext":"%s"}}\n' "$EVENT" "$ESCAPED_CONTENT"
