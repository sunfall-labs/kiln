'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const {spawn, spawnSync} = require('node:child_process');
const {once} = require('node:events');
const source = path.resolve(__dirname, '..');
const manifest = require('../package.json');

function fixture(t, body, version = manifest.version) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'kiln-launcher-'));
  t.after(() => fs.rmSync(root, {recursive:true, force:true}));
  fs.cpSync(source, path.join(root, 'launcher'), {recursive:true});
  const launcher = path.join(root, 'launcher/bin/kiln.cjs');
  if (body !== null) {
    const packageRoot = path.join(root, 'node_modules', `${manifest.name}-${process.platform}-${process.arch}`);
    fs.mkdirSync(path.join(packageRoot, 'bin'), {recursive:true});
    fs.writeFileSync(path.join(packageRoot, 'package.json'), JSON.stringify({version}));
    fs.writeFileSync(path.join(packageRoot, 'bin/kiln'), '#!/usr/bin/env node\n'+body, {mode:0o755});
  }
  return {root, launcher};
}

test('passes arguments, cwd, environment, output and nonzero exit status', t => {
  const {root, launcher} = fixture(t, 'console.log(JSON.stringify({args:process.argv.slice(2),cwd:process.cwd(),marker:process.env.KILN_LAUNCHER_TEST})); process.exit(17);');
  const result = spawnSync(process.execPath, [launcher, 'run', 'a file.ts', '--flag'], {
    cwd:root, encoding:'utf8', env:{...process.env, KILN_LAUNCHER_TEST:'inherited'},
  });
  assert.equal(result.status, 17);
  assert.deepEqual(JSON.parse(result.stdout), {args:['run','a file.ts','--flag'], cwd:root, marker:'inherited'});
});
test('missing optional dependency gives an actionable error', t => {
  const {launcher} = fixture(t, null);
  const result = spawnSync(process.execPath, [launcher], {encoding:'utf8'});
  assert.equal(result.status, 1);
  assert.match(result.stderr, /optional dependencies enabled/);
});
test('rejects a mismatched platform package version', t => {
  const {launcher} = fixture(t, 'process.exit(0);', '0.0.0');
  const result = spawnSync(process.execPath, [launcher], {encoding:'utf8'});
  assert.equal(result.status, 1);
  assert.match(result.stderr, /expected .*found 0\.0\.0/);
});
test('missing Mac package identifies the pending build', t => {
  const {launcher} = fixture(t, null);
  const result = spawnSync(process.execPath, ['-e',
    "Object.defineProperty(process,'platform',{value:'darwin'}); Object.defineProperty(process,'arch',{value:'arm64'}); require(process.argv[1]);",
    launcher], {encoding:'utf8'});
  assert.equal(result.status, 1);
  assert.match(result.stderr, /macOS arm64 build may still be pending/);
});
test('pending platform fails without downloading or executing anything', t => {
  const {launcher} = fixture(t, null);
  const packageFile = path.resolve(launcher, '../../package.json');
  fs.writeFileSync(packageFile, JSON.stringify({...manifest, optionalDependencies:{}}));
  const result = spawnSync(process.execPath, [launcher], {encoding:'utf8'});
  assert.equal(result.status, 1);
  assert.match(result.stderr, /has no npm binary/);
});
test('forwards termination and preserves the child signal', {timeout:10000}, async t => {
  const {launcher} = fixture(t, "console.log('ready'); setInterval(() => {}, 1000);");
  const child = spawn(process.execPath, [launcher], {stdio:['ignore','pipe','pipe']});
  t.after(() => child.kill('SIGKILL'));
  await once(child.stdout, 'data');
  const closed = once(child, 'close');
  child.kill('SIGTERM');
  assert.deepEqual(await closed, [null, 'SIGTERM']);
});
