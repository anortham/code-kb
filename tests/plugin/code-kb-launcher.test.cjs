'use strict';

const assert = require('node:assert/strict');
const childProcess = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const test = require('node:test');

const repoRoot = path.resolve(__dirname, '..', '..');
const launcherPath = path.join(repoRoot, 'bin', 'code-kb-launcher.cjs');
const launcher = require(launcherPath);

function tempDir(prefix) {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

async function listenOn(handler) {
  const server = http.createServer(handler);
  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { server, origin: `http://127.0.0.1:${server.address().port}` };
}

async function closeServer(server) {
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}

function fakeReleaseArchive(binaryName) {
  const stage = tempDir('code-kb-fake-release-');
  fs.writeFileSync(path.join(stage, binaryName), '#!/bin/sh\necho fake-code-kb "$@"\n', { mode: 0o755 });
  fs.writeFileSync(path.join(stage, 'julie-extract'), '#!/bin/sh\necho fake-julie\n', { mode: 0o755 });
  const archive = path.join(tempDir('code-kb-fake-archive-'), 'release.tar.gz');
  childProcess.execFileSync('tar', ['-czf', archive, '-C', stage, '.']);
  const bytes = fs.readFileSync(archive);
  return { bytes, sha256: crypto.createHash('sha256').update(bytes).digest('hex') };
}

function serveRelease(archive, sidecarText) {
  const requests = [];
  return listenOn((request, response) => {
    requests.push(request.url);
    if (request.url.endsWith('.sha256')) {
      response.end(sidecarText);
    } else if (request.url.endsWith('.tar.gz')) {
      response.writeHead(200, { 'content-length': String(archive.length) });
      response.end(archive);
    } else {
      response.statusCode = 404;
      response.end();
    }
  }).then((served) => ({ ...served, requests }));
}

test('detectPlatform maps the four release targets', () => {
  assert.deepEqual(launcher.detectPlatform('darwin', 'arm64'), {
    target: 'aarch64-apple-darwin', archiveExtension: '.tar.gz', binaryName: 'code-kb',
  });
  assert.deepEqual(launcher.detectPlatform('darwin', 'x64'), {
    target: 'x86_64-apple-darwin', archiveExtension: '.tar.gz', binaryName: 'code-kb',
  });
  assert.deepEqual(launcher.detectPlatform('linux', 'x64'), {
    target: 'x86_64-unknown-linux-gnu', archiveExtension: '.tar.gz', binaryName: 'code-kb',
  });
  assert.deepEqual(launcher.detectPlatform('win32', 'x64'), {
    target: 'x86_64-pc-windows-msvc', archiveExtension: '.zip', binaryName: 'code-kb.exe',
  });
  assert.throws(() => launcher.detectPlatform('linux', 'arm64'), /Unsupported code-kb platform: linux arm64/);
});

test('release asset names and URLs follow the Release Binaries workflow', () => {
  assert.equal(
    launcher.releaseArchiveName('1.0.2', 'x86_64-pc-windows-msvc', '.zip'),
    'code-kb-v1.0.2-x86_64-pc-windows-msvc.zip',
  );
  assert.equal(
    launcher.buildReleaseUrl('1.0.2', 'code-kb-v1.0.2-x86_64-unknown-linux-gnu.tar.gz'),
    'https://github.com/anortham/code-kb/releases/download/v1.0.2/code-kb-v1.0.2-x86_64-unknown-linux-gnu.tar.gz',
  );
});

test('the launcher reads the plugin version from the Claude plugin manifest', () => {
  const manifest = JSON.parse(fs.readFileSync(path.join(repoRoot, '.claude-plugin', 'plugin.json'), 'utf8'));
  assert.equal(launcher.readPluginVersion(repoRoot), manifest.version);
});

test('parseSha256Sidecar reads the checksum the workflow writes and rejects garbage', () => {
  const checksum = 'AB'.repeat(32);
  assert.equal(launcher.parseSha256Sidecar(`${checksum}  code-kb-v1.0.2-x.tar.gz\n`), checksum.toLowerCase());
  assert.throws(() => launcher.parseSha256Sidecar('not a checksum\n'), /SHA-256 sidecar/);
});

test('validateArchiveEntryNames rejects traversal and absolute paths', () => {
  assert.doesNotThrow(() => launcher.validateArchiveEntryNames(['./', './code-kb', './julie-extract']));
  for (const entry of ['../outside', 'a/../../b', '/tmp/x', 'C:\\x']) {
    assert.throws(() => launcher.validateArchiveEntryNames([entry]), /unsafe entry path/, entry);
  }
});

test('ensureBinary downloads, verifies, extracts once, and then hits the cache', { skip: process.platform === 'win32' }, async () => {
  const platformInfo = launcher.detectPlatform('linux', 'x64');
  const { bytes, sha256 } = fakeReleaseArchive('code-kb');
  const { server, origin, requests } = await serveRelease(bytes, `${sha256}  release.tar.gz\n`);
  const cacheRoot = tempDir('code-kb-cache-');

  try {
    const binary = await launcher.ensureBinary({ version: '9.9.9', platformInfo, cacheRoot, baseUrl: origin });
    assert.equal(binary, path.join(cacheRoot, '9.9.9', platformInfo.target, 'package', 'code-kb'));
    assert.ok(fs.existsSync(path.join(path.dirname(binary), 'julie-extract')));
    assert.equal(childProcess.execFileSync(binary, ['serve']).toString().trim(), 'fake-code-kb serve');

    const again = await launcher.ensureBinary({ version: '9.9.9', platformInfo, cacheRoot, baseUrl: origin });
    assert.equal(again, binary);
    assert.equal(requests.filter((url) => url.endsWith('.tar.gz')).length, 1);
  } finally {
    await closeServer(server);
  }
});

test('ensureBinary refuses an archive whose checksum does not match the sidecar', { skip: process.platform === 'win32' }, async () => {
  const platformInfo = launcher.detectPlatform('linux', 'x64');
  const { bytes } = fakeReleaseArchive('code-kb');
  const { server, origin } = await serveRelease(bytes, `${'0'.repeat(64)}  release.tar.gz\n`);
  const cacheRoot = tempDir('code-kb-cache-');

  try {
    await assert.rejects(
      () => launcher.ensureBinary({ version: '9.9.9', platformInfo, cacheRoot, baseUrl: origin }),
      /Checksum mismatch/,
    );
    assert.equal(fs.existsSync(path.join(cacheRoot, '9.9.9', platformInfo.target, 'package')), false);
  } finally {
    await closeServer(server);
  }
});

test('ensureBinary keeps only one older cached version', { skip: process.platform === 'win32' }, async () => {
  const platformInfo = launcher.detectPlatform('linux', 'x64');
  const { bytes, sha256 } = fakeReleaseArchive('code-kb');
  const { server, origin } = await serveRelease(bytes, `${sha256}  release.tar.gz\n`);
  const cacheRoot = tempDir('code-kb-cache-');
  for (const old of ['1.0.0', '1.0.1']) {
    fs.mkdirSync(path.join(cacheRoot, old, platformInfo.target, 'package'), { recursive: true });
  }
  const older = new Date(Date.now() - 60000);
  fs.utimesSync(path.join(cacheRoot, '1.0.0', platformInfo.target), older, older);

  try {
    await launcher.ensureBinary({ version: '1.0.2', platformInfo, cacheRoot, baseUrl: origin });
    assert.deepEqual(fs.readdirSync(cacheRoot).sort(), ['1.0.1', '1.0.2']);
  } finally {
    await closeServer(server);
  }
});

test('the launcher passes its arguments through to the binary named by CODE_KB_BIN', { skip: process.platform === 'win32' }, () => {
  const fake = path.join(tempDir('code-kb-bin-'), 'code-kb');
  fs.writeFileSync(fake, '#!/bin/sh\necho "args: $*"\nexit 7\n', { mode: 0o755 });

  const result = childProcess.spawnSync(process.execPath, [launcherPath, 'hook', 'SessionStart'], {
    env: { ...process.env, CODE_KB_BIN: fake },
    encoding: 'utf8',
  });

  assert.equal(result.stdout.trim(), 'args: hook SessionStart');
  assert.equal(result.status, 7);
});

test('the launcher runs the release build of its own checkout before downloading anything', { skip: process.platform === 'win32' }, () => {
  const checkout = tempDir('code-kb-checkout-');
  fs.mkdirSync(path.join(checkout, 'bin'));
  fs.mkdirSync(path.join(checkout, 'target', 'release'), { recursive: true });
  fs.copyFileSync(launcherPath, path.join(checkout, 'bin', 'code-kb-launcher.cjs'));
  fs.writeFileSync(path.join(checkout, 'target', 'release', 'code-kb'), '#!/bin/sh\necho "local: $*"\n', { mode: 0o755 });

  const result = childProcess.spawnSync(process.execPath, [path.join(checkout, 'bin', 'code-kb-launcher.cjs'), '--version'], {
    env: { ...process.env, CODE_KB_BIN: '', CODE_KB_HOME: tempDir('code-kb-home-') },
    encoding: 'utf8',
  });

  assert.equal(result.stdout.trim(), 'local: --version');
  assert.equal(result.status, 0);
  assert.equal(launcher.localReleaseBuild(checkout, 'code-kb'), path.join(checkout, 'target', 'release', 'code-kb'));
  assert.equal(launcher.localReleaseBuild(tempDir('code-kb-empty-'), 'code-kb'), null);
});

test('a binary at <CODE_KB_HOME>/bin/code-kb overrides the download and the checkout build', { skip: process.platform === 'win32' }, () => {
  const checkout = tempDir('code-kb-checkout-');
  fs.mkdirSync(path.join(checkout, 'bin'));
  fs.mkdirSync(path.join(checkout, 'target', 'release'), { recursive: true });
  fs.copyFileSync(launcherPath, path.join(checkout, 'bin', 'code-kb-launcher.cjs'));
  fs.writeFileSync(path.join(checkout, 'target', 'release', 'code-kb'), '#!/bin/sh\necho "checkout: $*"\n', { mode: 0o755 });
  const home = tempDir('code-kb-home-');
  fs.mkdirSync(path.join(home, 'bin'));
  fs.writeFileSync(path.join(home, 'bin', 'code-kb'), '#!/bin/sh\necho "override: $*"\n', { mode: 0o755 });

  const result = childProcess.spawnSync(process.execPath, [path.join(checkout, 'bin', 'code-kb-launcher.cjs'), 'serve'], {
    env: { ...process.env, CODE_KB_BIN: '', CODE_KB_HOME: home },
    encoding: 'utf8',
  });

  assert.equal(result.stdout.trim(), 'override: serve');
  assert.equal(launcher.overrideBinary('code-kb', { CODE_KB_HOME: home }), path.join(home, 'bin', 'code-kb'));
  assert.equal(launcher.overrideBinary('code-kb', { CODE_KB_HOME: tempDir('code-kb-empty-home-') }), null);
});

