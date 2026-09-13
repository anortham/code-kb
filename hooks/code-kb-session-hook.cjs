#!/usr/bin/env node
// code-kb — SessionStart and SubagentStart activation hook
// Injects token-dense code-intelligence routing instructions into the agent's context.

const fs = require('fs');
const path = require('path');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);
const isQoder = !isCopilot && !isCodex && Boolean(process.env.QODER_SESSION_ID);

function getRoutingContext() {
  const mdPath = path.join(__dirname, 'code-kb-routing-block.md');
  try {
    return fs.readFileSync(mdPath, 'utf8').trim();
  } catch (err) {
    return [
      '## Code Intelligence: Always use code-kb MCP tools',
      'Use code-kb MCP tools (codebase_outline, file_skeleton, find_symbol, search_symbols, get_context_slice, get_symbol_body, find_references, find_structural_facts, replace_symbol_body) for symbol search, interface discovery, and editing.',
      'Do NOT run grep or cat to search symbols or read interfaces.',
      'Never supply workspace or repo_path parameters; workspace root is bound automatically.'
    ].join('\n');
  }
}

function determineEvent() {
  // Check CLI arguments first (e.g. node script.js SessionStart)
  for (const arg of process.argv.slice(2)) {
    if (arg === 'SessionStart' || arg === 'SubagentStart') {
      return arg;
    }
  }
  return 'SessionStart';
}

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify(
      event === 'SessionStart' && context ? { additionalContext: context } : {}
    ));
    return;
  }

  if (isCodex || isQoder) {
    const output = {
      hookSpecificOutput: {
        hookEventName: event,
        additionalContext: context,
      }
    };
    process.stdout.write(JSON.stringify(output));
    return;
  }

  // Claude Code
  if (event === 'SubagentStart') {
    process.stdout.write(JSON.stringify({
      hookSpecificOutput: {
        hookEventName: event,
        additionalContext: context,
      }
    }));
    return;
  }

  // SessionStart in Claude Code accepts raw text or hookSpecificOutput.
  // Using JSON hookSpecificOutput ensures consistency across agent hosts.
  process.stdout.write(JSON.stringify({
    hookSpecificOutput: {
      hookEventName: event,
      additionalContext: context,
    }
  }));
}

try {
  const event = determineEvent();
  const context = getRoutingContext();
  writeHookOutput(event, context);
} catch (e) {
  // Best effort: never block session launch on hook error
  process.exit(0);
}
