// What every callable on the global object calls itself.
//
// WHY THIS IS A SEPARATE PROBE FROM `global-surface.ts`. That one prints `typeof` for every name
// the realm publishes, which catches a global that exists in one engine and not the other. It
// does not read `.name`, and `.name` is where nine wrong answers sat undisturbed: the native
// backend reported "anonymous" for `AbortController`, `AbortSignal` and `TextEncoder`, the empty
// string for `setImmediate` and `clearImmediate`, and the interpreter reported its own Rust
// identifiers -- "nodeBufferGlobal" for `Buffer`, "Url" for `URL`. Every one of those is a string
// a program can read and none of them is a name anybody could have asked for.
//
// A `.name` is not decoration. It is what a decorator preserves, what a stack trace prints, what
// a dependency-injection container keys on, and what `fn.name === "Buffer"` tests. A leaked
// implementation identifier there is a wrong answer of exactly the shape this corpus exists to
// find, and it is *cheap* to find: the realm publishes its own list, so nothing here names a
// built-in and a global added to either engine is covered the day it lands.
//
// `$262` is skipped for the reason `global-surface.ts` skips it: those are the Test262 host hooks
// the runner installs, a statement about the host rather than about the language.
const names = Object.getOwnPropertyNames(globalThis)
  .filter((name: string) => !name.startsWith("$262"))
  .sort();
for (const privateHook of [
  "__kilnCryptoKeyExport",
  "__kilnCryptoKeyFree",
  "__kilnCryptoKeyImport",
  "__kilnCryptoKeyInfo",
  "__kilnCryptoKeyPublic",
  "__kilnDecorateCommonJsRequire",
  "__kilnHostModuleCreateRequire",
  "__kilnHostModuleRequireResolve",
  "__kilnNativeImportMeta",
]) {
  if (Object.prototype.hasOwnProperty.call(globalThis, privateHook)) {
    throw new Error(`runtime hook ${privateHook} leaked onto globalThis`);
  }
}
const globals = globalThis as Record<string, unknown>;
let callables = 0;
for (const name of names) {
  const value = globals[name];
  if (typeof value !== "function") continue;
  callables += 1;
  console.log(name, JSON.stringify((value as { name: unknown }).name));
}
// A count, so that a realm which stopped publishing its callables -- and therefore stopped being
// asked anything at all -- reads as a change rather than as a clean run of an empty loop.
console.log("callables", callables);
