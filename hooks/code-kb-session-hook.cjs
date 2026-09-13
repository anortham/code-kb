#!/usr/bin/env node
// code-kb — SessionStart and SubagentStart activation hook
// Injects token-dense code-intelligence routing instructions into the agent's context.

const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');

const isCopilot = Boolean(process.env.COPILOT_PLUGIN_DATA);
const isCodex = !isCopilot && Boolean(process.env.PLUGIN_DATA);
const isQoder = !isCopilot && !isCodex && Boolean(process.env.QODER_SESSION_ID);

function determineEvent() {
  for (const arg of process.argv.slice(2)) {
    if (arg === 'SessionStart' || arg === 'SubagentStart') {
      return arg;
    }
  }
  return 'SessionStart';
}

const event = determineEvent();

// Try native code-kb CLI first if available
try {
  const stdout = execSync(`code-kb hook ${event}`, { stdio: ['ignore', 'pipe', 'ignore'], timeout: 2000 });
  if (stdout && stdout.length > 0) {
    process.stdout.write(stdout);
    process.exit(0);
  }
} catch (_) {
  // Fall back to JS logic below
}

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

function writeHookOutput(event, context) {
  if (isCopilot) {
    process.stdout.write(JSON.stringify(
      event === 'SessionStart' && context ? { additionalContext: context } : {}
    ));
    return;
  }

  const output = {
    hookSpecificOutput: {
      hookEventName: event,
      additionalContext: context,
    }
  };
  process.stdout.write(JSON.stringify(output));
}

try {
  const context = getRoutingContext();
  writeHookOutput(event, context);
} catch (e) {
  process.exit(0);
}
