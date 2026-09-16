'use strict';

const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const root = path.resolve(__dirname, '..', '..');
const read = (file) => JSON.parse(fs.readFileSync(path.join(root, file), 'utf8'));

test('the Claude plugin starts the server and hooks through the launcher', () => {
  const plugin = read('.claude-plugin/plugin.json');
  assert.deepEqual(plugin.mcpServers['code-kb'], {
    command: 'node',
    args: ['${CLAUDE_PLUGIN_ROOT}/bin/code-kb-launcher.cjs', 'serve'],
  });
  assert.equal(plugin.hooks, './hooks/claude-codex-hooks.json');

  const hooks = read('hooks/claude-codex-hooks.json').hooks;
  for (const event of ['SessionStart', 'SubagentStart']) {
    const command = hooks[event][0].hooks[0].command;
    assert.equal(command, `node "\${CLAUDE_PLUGIN_ROOT}/bin/code-kb-launcher.cjs" hook ${event}`);
    assert.ok(hooks[event][0].hooks[0].timeout >= 60, `${event} hook must allow a first-run download`);
  }
});

test('the Codex plugin starts the server through the launcher', () => {
  const plugin = read('.codex-plugin/plugin.json');
  assert.equal(plugin.mcpServers, './.mcp.json');
  assert.equal(plugin.hooks, './hooks/claude-codex-hooks.json');
  assert.deepEqual(read('.mcp.json').mcpServers['code-kb'], {
    command: 'node',
    args: ['./bin/code-kb-launcher.cjs', 'serve'],
    cwd: '.',
  });
});

test('the Antigravity plugin starts the server and hook through the launcher', () => {
  assert.equal(read('plugin.json').name, 'code-kb');
  assert.equal(read('plugin.json').$schema, undefined, 'a $schema on the root manifest makes Codex skip the .codex-plugin hooks');
  assert.deepEqual(read('mcp_config.json').mcpServers['code-kb'], {
    command: 'node',
    args: ['./bin/code-kb-launcher.cjs', 'serve'],
  });
  assert.deepEqual(read('hooks.json')['code-kb'].PreInvocation, [
    { command: 'node ./bin/code-kb-launcher.cjs hook PreInvocation', timeout: 60, type: 'command' },
  ]);
});

test('every manifest carries the same version', () => {
  const versions = new Set([
    read('.claude-plugin/plugin.json').version,
    read('.codex-plugin/plugin.json').version,
    read('plugin.json').version,
    read('.claude-plugin/marketplace.json').plugins[0].version,
  ]);
  assert.equal(versions.size, 1, [...versions].join(', '));
});
