#!/usr/bin/env node
'use strict';

const {spawn} = require('node:child_process');
const path = require('node:path');
const manifest = require('../package.json');
const platform = `${process.platform}-${process.arch}`;
const packageName = `${manifest.name}-${platform}`;

function fail(message) {
  console.error(`kiln: ${message}`);
  process.exit(1);
}

if (!Object.hasOwn(manifest.optionalDependencies, packageName)) {
  fail(`version ${manifest.version} has no npm binary for ${platform}. `+
    (platform === 'darwin-arm64' ? 'The macOS arm64 build is pending. ' : '')+
    'See https://github.com/sunfall-labs/kiln/releases for supported builds.');
}

let binary;
try {
  const packageFile = require.resolve(`${packageName}/package.json`);
  const installed = require(packageFile);
  if (installed.version !== manifest.version) {
    fail(`expected ${packageName}@${manifest.version}, found ${installed.version}. Reinstall kiln.`);
  }
  binary = path.join(path.dirname(packageFile), 'bin', 'kiln');
} catch (error) {
  if (error.code !== 'MODULE_NOT_FOUND') fail(error.message);
  fail(`missing ${packageName}@${manifest.version}. Reinstall with optional dependencies enabled `+
    '(avoid --omit=optional or --no-optional). '+
    (platform === 'darwin-arm64'
      ? 'Check the release notes: the macOS arm64 build may still be pending; reinstall after it is published.'
      : 'Linux builds require glibc.'));
}

const child = spawn(binary, process.argv.slice(2), {stdio:'inherit'});
const handlers = new Map();
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  const forward = () => child.kill(signal);
  handlers.set(signal, forward);
  process.on(signal, forward);
}
child.on('error', error => fail(`cannot start ${binary}: ${error.message}`));
child.on('close', (code, signal) => {
  for (const [name, handler] of handlers) process.removeListener(name, handler);
  if (signal) process.kill(process.pid, signal);
  else process.exit(code === null ? 1 : code);
});
