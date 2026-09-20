#!/usr/bin/env node
'use strict';

const childProcess = require('node:child_process');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const https = require('node:https');
const os = require('node:os');
const path = require('node:path');
const { pipeline } = require('node:stream/promises');

const REPOSITORY = 'anortham/code-kb';
const CONNECT_TIMEOUT_MS = 30000;
const IDLE_TIMEOUT_MS = 15000;
const DOWNLOAD_ATTEMPTS = 3;
const RETAINED_OLD_VERSIONS = 1;

function log(message) {
  process.stderr.write(`code-kb-launcher: ${message}\n`);
}

function detectPlatform(platform = os.platform(), arch = os.arch()) {
  const targets = {
    'darwin:arm64': { target: 'aarch64-apple-darwin', archiveExtension: '.tar.gz', binaryName: 'code-kb' },
    'darwin:x64': { target: 'x86_64-apple-darwin', archiveExtension: '.tar.gz', binaryName: 'code-kb' },
    'linux:x64': { target: 'x86_64-unknown-linux-gnu', archiveExtension: '.tar.gz', binaryName: 'code-kb' },
    'linux:arm64': { target: 'aarch64-unknown-linux-gnu', archiveExtension: '.tar.gz', binaryName: 'code-kb' },
    'win32:x64': { target: 'x86_64-pc-windows-msvc', archiveExtension: '.zip', binaryName: 'code-kb.exe' },
    'win32:arm64': { target: 'aarch64-pc-windows-msvc', archiveExtension: '.zip', binaryName: 'code-kb.exe' },
  };
  const found = targets[`${platform}:${arch}`];
  if (!found) {
    throw new Error(`Unsupported code-kb platform: ${platform} ${arch}. Install a release archive by hand (see README).`);
  }
  return found;
}

function releaseArchiveName(version, target, archiveExtension) {
  return `code-kb-v${version}-${target}${archiveExtension}`;
}

function buildReleaseUrl(version, assetName) {
  return `https://github.com/${REPOSITORY}/releases/download/v${version}/${encodeURIComponent(assetName)}`;
}

function readPluginVersion(pluginRoot) {
  const manifest = JSON.parse(fs.readFileSync(path.join(pluginRoot, '.claude-plugin', 'plugin.json'), 'utf8'));
  return String(manifest.version).replace(/^v/, '');
}

function parseSha256Sidecar(text) {
  const match = String(text).match(/\b([a-fA-F0-9]{64})\b/);
  if (!match) {
    throw new Error('Invalid SHA-256 sidecar.');
  }
  return match[1].toLowerCase();
}

function validateArchiveEntryNames(entries) {
  for (const rawEntry of entries) {
    const entry = String(rawEntry || '').trim();
    const normalized = entry.replace(/\\/g, '/').replace(/^\.\/+/, '');
    const parts = normalized.split('/').filter(Boolean);
    if (normalized.startsWith('/') || /^[A-Za-z]:\//.test(normalized) || parts.includes('..')) {
      throw new Error(`Release archive contains unsafe entry path: ${entry}`);
    }
  }
}

function defaultCacheRoot(env = process.env) {
  const home = env.CODE_KB_HOME ? path.resolve(env.CODE_KB_HOME) : path.join(os.homedir(), '.code-kb');
  return path.join(home, 'dist');
}

function httpClientFor(url) {
  const parsed = new URL(url);
  if (parsed.protocol === 'https:') {
    return https;
  }
  if (parsed.protocol === 'http:' && ['127.0.0.1', '::1', 'localhost'].includes(parsed.hostname)) {
    return http;
  }
  throw new Error(`Refusing to download over ${parsed.protocol}//${parsed.hostname}`);
}

function downloadFile(url, destination, redirects = 0) {
  if (redirects > 5) {
    return Promise.reject(new Error(`Too many redirects while downloading ${url}`));
  }
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  const temporary = `${destination}.tmp-${process.pid}`;

  return new Promise((resolve, reject) => {
    let idleTimer = null;
    const request = httpClientFor(url).get(url, { timeout: CONNECT_TIMEOUT_MS }, (response) => {
      if (response.statusCode >= 300 && response.statusCode < 400 && response.headers.location) {
        response.resume();
        downloadFile(new URL(response.headers.location, url).toString(), destination, redirects + 1).then(resolve, reject);
        return;
      }
      if (response.statusCode !== 200) {
        response.resume();
        request.destroy();
        reject(new Error(`Failed to download ${url}: HTTP ${response.statusCode}`));
        return;
      }
      const armIdle = () => {
        clearTimeout(idleTimer);
        idleTimer = setTimeout(() => request.destroy(new Error(`Download stalled: ${url}`)), IDLE_TIMEOUT_MS);
      };
      response.on('data', armIdle);
      armIdle();
      pipeline(response, fs.createWriteStream(temporary, { mode: 0o644 }))
        .then(() => {
          clearTimeout(idleTimer);
          fs.renameSync(temporary, destination);
          resolve();
        })
        .catch((error) => {
          clearTimeout(idleTimer);
          reject(error);
        });
    });
    request.on('timeout', () => request.destroy(new Error(`No response within ${CONNECT_TIMEOUT_MS}ms: ${url}`)));
    request.on('error', reject);
  }).catch((error) => {
    fs.rmSync(temporary, { force: true });
    throw error;
  });
}

async function downloadWithRetry(url, destination) {
  let lastError;
  for (let attempt = 1; attempt <= DOWNLOAD_ATTEMPTS; attempt++) {
    try {
      return await downloadFile(url, destination);
    } catch (error) {
      lastError = error;
      if (attempt < DOWNLOAD_ATTEMPTS) {
        log(`download attempt ${attempt}/${DOWNLOAD_ATTEMPTS} failed, retrying: ${error.message}`);
      }
    }
  }
  throw lastError;
}

function fileSha256(filePath) {
  return new Promise((resolve, reject) => {
    const hash = crypto.createHash('sha256');
    fs.createReadStream(filePath)
      .on('data', (chunk) => hash.update(chunk))
      .on('error', reject)
      .on('end', () => resolve(hash.digest('hex')));
  });
}

function tarBinary() {
  if (process.platform !== 'win32') {
    return 'tar';
  }
  const systemTar = path.join(process.env.SystemRoot || 'C:\\Windows', 'System32', 'tar.exe');
  if (!fs.existsSync(systemTar)) {
    throw new Error(`${systemTar} is required to unpack the release archive (Windows 10 build 17063 or later).`);
  }
  return systemTar;
}

function run(command, args) {
  const result = childProcess.spawnSync(command, args, { encoding: 'utf8', windowsHide: true });
  if (result.error || result.status !== 0) {
    throw new Error(`${command} ${args.join(' ')} failed: ${result.error ? result.error.message : (result.stderr || '').trim() || `exit ${result.status}`}`);
  }
  return result.stdout;
}

function extractArchive(archivePath, archiveExtension, destination) {
  const tar = tarBinary();
  const gz = archiveExtension === '.tar.gz';
  const entries = run(tar, [gz ? '-tzf' : '-tf', archivePath]).split(/\r?\n/).filter(Boolean);
  validateArchiveEntryNames(entries);
  fs.mkdirSync(destination, { recursive: true });
  run(tar, [gz ? '-xzf' : '-xf', archivePath, '-C', destination]);
}

function pruneOldVersions(cacheRoot, currentVersion, target) {
  let versions;
  try {
    versions = fs.readdirSync(cacheRoot, { withFileTypes: true })
      .filter((entry) => entry.isDirectory() && entry.name !== currentVersion)
      .map((entry) => {
        let usedAt = 0;
        try {
          usedAt = fs.statSync(path.join(cacheRoot, entry.name, target)).mtimeMs;
        } catch {}
        return { name: entry.name, usedAt };
      })
      .sort((left, right) => right.usedAt - left.usedAt);
  } catch {
    return;
  }
  for (const version of versions.slice(RETAINED_OLD_VERSIONS)) {
    try {
      fs.rmSync(path.join(cacheRoot, version.name), { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
    } catch (error) {
      log(`could not remove cached version ${version.name}: ${error.message}`);
    }
  }
}

// Returns the path of the code-kb binary for `version`, downloading and unpacking the release once per version.
async function ensureBinary({ version, platformInfo, cacheRoot = defaultCacheRoot(), baseUrl = null }) {
  const targetRoot = path.join(cacheRoot, version, platformInfo.target);
  const packageDir = path.join(targetRoot, 'package');
  const binaryPath = path.join(packageDir, platformInfo.binaryName);
  if (fs.existsSync(binaryPath)) {
    return binaryPath;
  }

  const archiveName = releaseArchiveName(version, platformInfo.target, platformInfo.archiveExtension);
  const archiveUrl = baseUrl ? `${baseUrl}/${archiveName}` : buildReleaseUrl(version, archiveName);
  const downloadDir = path.join(targetRoot, 'downloads');
  const archivePath = path.join(downloadDir, archiveName);
  const sidecarPath = `${archivePath}.sha256`;
  const stageDir = path.join(targetRoot, `stage-${process.pid}`);

  log(`first launch of code-kb ${version}: downloading ${archiveUrl}`);
  try {
    await downloadWithRetry(`${archiveUrl}.sha256`, sidecarPath);
    const expected = parseSha256Sidecar(fs.readFileSync(sidecarPath, 'utf8'));
    await downloadWithRetry(archiveUrl, archivePath);
    const actual = await fileSha256(archivePath);
    if (actual !== expected) {
      throw new Error(`Checksum mismatch for ${archiveName}: expected ${expected}, got ${actual}`);
    }

    fs.rmSync(stageDir, { recursive: true, force: true });
    extractArchive(archivePath, platformInfo.archiveExtension, stageDir);
    if (!fs.existsSync(path.join(stageDir, platformInfo.binaryName))) {
      throw new Error(`${archiveName} does not contain ${platformInfo.binaryName}`);
    }
    if (process.platform !== 'win32') {
      for (const name of fs.readdirSync(stageDir)) {
        fs.chmodSync(path.join(stageDir, name), 0o755);
      }
    }
    if (process.platform === 'darwin') {
      childProcess.spawnSync('xattr', ['-dr', 'com.apple.quarantine', stageDir], { stdio: 'ignore' });
    }
    try {
      fs.renameSync(stageDir, packageDir);
    } catch (error) {
      if (!fs.existsSync(binaryPath)) {
        throw error;
      }
    }
    log(`ready: ${binaryPath}`);
    pruneOldVersions(cacheRoot, version, platformInfo.target);
    return binaryPath;
  } finally {
    fs.rmSync(stageDir, { recursive: true, force: true });
    fs.rmSync(downloadDir, { recursive: true, force: true });
  }
}

function runBinary(binaryPath, args) {
  if (require.main === module && typeof process.execve === 'function') {
    try {
      process.execve(binaryPath, [binaryPath, ...args], process.env);
    } catch {
      // Fall back to childProcess.spawn if execve fails
    }
  }
  const child = childProcess.spawn(binaryPath, args, { stdio: 'inherit', windowsHide: true });
  for (const signal of ['SIGINT', 'SIGTERM']) {
    process.once(signal, () => child.kill(signal));
  }
  return new Promise((resolve) => {
    child.on('exit', (code, signal) => resolve(signal ? 1 : code ?? 1));
    child.on('error', (error) => {
      log(`failed to start ${binaryPath}: ${error.message}`);
      resolve(1);
    });
  });
}

// Harnesses that clone plugins into a cache (Codex) or drop the environment of MCP servers
// cannot see CODE_KB_BIN or a checkout build, so a binary or symlink at ~/.code-kb/bin/code-kb
// overrides the download in every harness.
function overrideBinary(binaryName, env = process.env) {
  const candidate = path.join(path.dirname(defaultCacheRoot(env)), 'bin', binaryName);
  return fs.existsSync(candidate) ? candidate : null;
}

// A plugin installed from a source checkout runs that checkout's release build, so
// `cargo build --release` plus a session restart is the whole dev loop.
function localReleaseBuild(pluginRoot, binaryName) {
  const candidate = path.join(pluginRoot, 'target', 'release', binaryName);
  return fs.existsSync(candidate) ? candidate : null;
}

async function main() {
  const args = process.argv.slice(2);
  if (process.env.CODE_KB_BIN) {
    return runBinary(process.env.CODE_KB_BIN, args);
  }
  const pluginRoot = path.resolve(__dirname, '..');
  const platformInfo = detectPlatform();
  const preferred = overrideBinary(platformInfo.binaryName) || localReleaseBuild(pluginRoot, platformInfo.binaryName);
  if (preferred) {
    return runBinary(preferred, args);
  }
  const binaryPath = await ensureBinary({
    version: process.env.CODE_KB_VERSION || readPluginVersion(pluginRoot),
    platformInfo,
  });
  return runBinary(binaryPath, args);
}

module.exports = {
  buildReleaseUrl,
  defaultCacheRoot,
  detectPlatform,
  ensureBinary,
  localReleaseBuild,
  overrideBinary,
  parseSha256Sidecar,
  readPluginVersion,
  releaseArchiveName,
  runBinary,
  validateArchiveEntryNames,
};

if (require.main === module) {
  main()
    .then((code) => process.exit(code))
    .catch((error) => {
      log(error.message);
      process.exit(1);
    });
}
