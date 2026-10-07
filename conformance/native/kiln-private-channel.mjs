// ADR 0068: with extensions on (the default), a program that uses only Node's surface sees nothing
// of kiln's private channels. No `Symbol.for("kiln...")` key on a built-in, no `__kilnPrivate` or
// `__kiln*` global, no kiln-only member on a Node class, and kiln's own globals are not enumerable.
// The engines lend each builtin module a realm-private object only while that module initializes.
import * as wt from 'node:worker_threads';

const hasKiln = (key) => typeof key === 'symbol' && /kiln/i.test(`${Symbol.keyFor(key) ?? ''}${key.description ?? ''}`);
const leaks = [];
const check = (label, object) => {
  if (object === null || (typeof object !== 'object' && typeof object !== 'function')) return;
  for (const key of Reflect.ownKeys(object)) {
    if (hasKiln(key)) leaks.push(`${label}[${String(key)}]`);
  }
};
const named = {
  MessagePort: wt.MessagePort, 'MessagePort.prototype': wt.MessagePort.prototype,
  MessageChannel: wt.MessageChannel, Worker: wt.Worker, 'Worker.prototype': wt.Worker.prototype,
  'worker_threads': wt, structuredClone, URL, 'URL.prototype': URL.prototype,
  URLSearchParams, 'URLSearchParams.prototype': URLSearchParams.prototype,
  globalThis, process, console, Object, Array, Symbol, Promise, Atomics,
};
for (const [label, object] of Object.entries(named)) check(label, object);
// Every global's value, so a channel hung on some class nobody listed shows.
for (const name of Object.getOwnPropertyNames(globalThis)) {
  let value;
  try { value = globalThis[name]; } catch { continue; }
  check(`globalThis.${name}`, value);
}
console.log('kiln-keyed symbols:', leaks.length === 0 ? 'none' : leaks.join(', '));

const names = Object.getOwnPropertyNames(globalThis).filter((name) => /^__kiln/.test(name));
console.log('__kiln globals:', names.length === 0 ? 'none' : names.join(','));
console.log('__kilnPrivate in globalThis:', '__kilnPrivate' in globalThis, typeof globalThis.__kilnPrivate);
console.log('Worker.prototype members:', Object.getOwnPropertyNames(wt.Worker.prototype).filter((name) => /kiln|actor/i.test(name)).length);

// `Kiln` and `Bun` are kiln's own globals: present, but never visible to a walk of the global.
const walked = [];
for (const key in globalThis) walked.push(key);
console.log('enumerable kiln globals:', walked.filter((key) => key === 'Kiln' || key === 'Bun').length,
  Object.keys(globalThis).filter((key) => key === 'Kiln' || key === 'Bun').length);

// An engine-internal module is not importable by a program, whatever the specifier's spelling.
for (const load of [() => import('kiln:actors/core'), () => import('kiln:actors/worker')]) {
  try { await load(); console.log('internal import: resolved'); }
  catch (error) { console.log('internal import:', error.name, /is internal to kiln:actors/.test(error.message)); }
}
try { new wt.Worker('kiln:actors/worker'); console.log('actor entry: started'); }
catch (error) { console.log('actor entry:', error.name, error.code); }
console.log('getBuiltinModule:', process.getBuiltinModule('kiln:actors/core'));
