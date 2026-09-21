'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const root = path.resolve(__dirname, '..', '..');
const read = (file) => JSON.parse(fs.readFileSync(path.join(root, file), 'utf8'));

function releaseGateScript() {
  const workflow = fs.readFileSync(path.join(root, '.github/workflows/release-binaries.yml'), 'utf8');
  const match = workflow.match(/ {8}run: \|\n((?: {10}.*\n)+)/);
  assert.ok(match, 'release workflow must contain the verification script');
  return match[1].replace(/^ {10}/gm, '');
}

function runReleaseGate(overrides = {}) {
  const version = read('.claude-plugin/plugin.json').version;
  const temp = fs.mkdtempSync(path.join(os.tmpdir(), 'code-kb-release-gate-'));
  const script = path.join(temp, 'gate.sh');
  const git = path.join(temp, 'git');
  const gh = path.join(temp, 'gh');
  const jq = path.join(temp, 'jq');
  fs.writeFileSync(script, releaseGateScript());
  fs.writeFileSync(git, '#!/bin/sh\nif [ "$1" != rev-parse ] || [ "$2" != "${GITHUB_SHA}^{commit}" ]; then exit 2; fi\nprintf "%s\\n" "$MOCK_COMMIT"\n');
  fs.writeFileSync(gh, '#!/bin/sh\ncase "$*" in *"--commit $MOCK_COMMIT"*) printf "%s\\n" "$MOCK_CI_STATUS";; *) exit 2;; esac\n');
  fs.writeFileSync(jq, '#!/bin/sh\nlast=""\nfor arg in "$@"; do last="$arg"; done\ncase "$last" in .claude-plugin/plugin.json) printf "%s\\n" "$MOCK_CLAUDE_VERSION";; .codex-plugin/plugin.json) printf "%s\\n" "$MOCK_CODEX_VERSION";; .claude-plugin/marketplace.json) printf "%s\\n" "$MOCK_MARKETPLACE_VERSION";; plugin.json) printf "%s\\n" "$MOCK_ROOT_VERSION";; *) exit 2;; esac\n');
  for (const file of [git, gh, jq]) fs.chmodSync(file, 0o755);
  try {
    return childProcess.spawnSync('bash', [script], {
      cwd: root,
      encoding: 'utf8',
      env: {
        ...process.env,
        ...overrides,
        PATH: `${temp}:${process.env.PATH}`,
        EVENT_NAME: overrides.EVENT_NAME || 'workflow_dispatch',
        REF_NAME: overrides.REF_NAME || 'main',
        DISPATCH_VERSION: overrides.DISPATCH_VERSION || version,
        GITHUB_SHA: overrides.GITHUB_SHA || 'annotated-tag-object',
        MOCK_COMMIT: overrides.MOCK_COMMIT || 'peeled-commit',
        MOCK_CI_STATUS: overrides.MOCK_CI_STATUS || 'completed/success',
        MOCK_CLAUDE_VERSION: overrides.MOCK_CLAUDE_VERSION || version,
        MOCK_CODEX_VERSION: overrides.MOCK_CODEX_VERSION || version,
        MOCK_MARKETPLACE_VERSION: overrides.MOCK_MARKETPLACE_VERSION || version,
        MOCK_ROOT_VERSION: overrides.MOCK_ROOT_VERSION || version,
      },
    });
  } finally {
    fs.rmSync(temp, { recursive: true, force: true });
  }
}

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

test('the release gate requires green exact-commit CI and matching versions', { skip: process.platform === 'win32' && 'uses POSIX shell fixtures for an Ubuntu workflow' }, () => {
  const version = read('.claude-plugin/plugin.json').version;
  assert.equal(runReleaseGate().status, 0);
  assert.equal(runReleaseGate({ EVENT_NAME: 'push', REF_NAME: `v${version}` }).status, 0);
  for (const status of ['completed/failure', 'in_progress/null', 'null/null']) {
    assert.equal(runReleaseGate({ MOCK_CI_STATUS: status }).status, 1);
  }
  assert.equal(runReleaseGate({ DISPATCH_VERSION: `${version}.mismatch` }).status, 1);
  assert.equal(runReleaseGate({ EVENT_NAME: 'push', REF_NAME: `v${version}.mismatch` }).status, 1);
  assert.equal(runReleaseGate({ MOCK_MARKETPLACE_VERSION: `${version}.mismatch` }).status, 1);
});

test('the manifest version reaches the crate, the workflow, the site, and the release notes', () => {
  const version = read('.claude-plugin/plugin.json').version;
  const text = (file) => fs.readFileSync(path.join(root, file), 'utf8');

  assert.match(text('Cargo.toml'), new RegExp(`^version = "${version}"$`, 'm'));
  assert.match(text('crates/code-kb-cli/Cargo.toml'), new RegExp(`code-kb-core = \\{ version = "${version}"`));
  for (const crate of ['code-kb-cli', 'code-kb-core']) {
    assert.match(text('Cargo.lock'), new RegExp(`name = "${crate}"\\nversion = "${version}"`));
  }
  assert.match(text('.github/workflows/release-binaries.yml'), new RegExp(`default: "${version}"`));
  assert.match(text('docs/site/index.html'), new RegExp(`<span class="version">v${version}</span>`));
  assert.ok(fs.existsSync(path.join(root, `docs/release-notes/v${version}.md`)));
});
