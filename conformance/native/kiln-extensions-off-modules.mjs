// native-env: KILN_EXTENSIONS=0
//
// ADR 0068 rule 4, for modules and globals: with extensions off, every `kiln:` module -- the
// public ones and the engine-internal ones -- refuses by name with one error class in both engines
// (a TypeError, which is what a failed `import()` rejects with), and none of kiln's globals or
// private hooks exist. The first line is read by `scripts/native.sh`, which runs both engines with
// that environment. Static imports are covered by `kiln-cli`'s `extensions_off` test.
import { Worker } from 'node:worker_threads';

for (const load of [
  () => import('kiln:shared'),
  () => import('kiln:actors'),
  () => import('kiln:process'),
]) {
  try { await load(); console.log('imported'); }
  catch (error) { console.log(error.constructor === TypeError, error.name, error.message); }
}
// Internal modules are refused to programs whether or not extensions are on.
for (const load of [() => import('kiln:actors/core'), () => import('kiln:actors/worker')]) {
  try { await load(); console.log('imported'); }
  catch (error) { console.log(error.constructor === TypeError, /is internal to kiln:actors/.test(error.message)); }
}

console.log('Kiln, Bun:', typeof Kiln, typeof Bun, 'Kiln' in globalThis, 'Bun' in globalThis);
const walked = [];
for (const key in globalThis) walked.push(key);
console.log('walk:', walked.filter((key) => /kiln|bun/i.test(key)).length);
console.log('names:', Object.getOwnPropertyNames(globalThis).filter((key) => /kiln|bun/i.test(key)).length);
console.log('process hooks:', '__kilnHostProcessStart' in globalThis, '__kilnHostProcessCancel' in globalThis,
  '__kilnHostProcessStart' in process, '__kilnHostProcessCancel' in process);
console.log('private:', '__kilnPrivate' in globalThis, typeof globalThis.__kilnPrivate,
  Object.getOwnPropertyNames(globalThis).includes('__kilnPrivate'));
console.log('shared hooks:', '__kilnShare' in globalThis, '__kilnSharedIs' in globalThis);

// The actor worker entry is not a path a program can start.
try { new Worker('kiln:actors/worker'); console.log('actor entry: started'); }
catch (error) { console.log('actor entry:', error.name, error.code); }
console.log('getBuiltinModule:', process.getBuiltinModule('kiln:actors/core'));
