// Every static the realm hangs off `Number`, and the exact bytes of each one's value.
//
// WHY THE LIST IS WRITTEN OUT. The other surface probes in this directory ask the realm for its
// own names -- `global-surface.ts` uses `Object.getOwnPropertyNames(globalThis)` so that a global
// added to one engine and not the other changes the row count without anyone remembering to edit
// the probe. This one cannot yet: the native backend refuses `Object.getOwnPropertyNames` when
// the receiver is a builtin constructor, because those builtins are dispatched by name and have
// no own-property table to walk. Until they do, the names here are written out, and the list is
// only as good as this comment is at telling you to extend it.
//
// WHY IT EXISTS AT ALL. The interpreter carried fifteen of these; the native prelude carried
// five. `Number.MAX_VALUE` is the first property a bundled `zod` reads, so the two that were
// missing refused the whole module -- and nothing caught it, because no probe asked. Values are
// printed, not just typeof: a static that is present but wrong is a silent wrong answer, which is
// worse than a refusal.

const names = [
  "EPSILON",
  "MAX_SAFE_INTEGER",
  "MAX_VALUE",
  "MIN_SAFE_INTEGER",
  "MIN_VALUE",
  "NEGATIVE_INFINITY",
  "NaN",
  "POSITIVE_INFINITY",
  "isFinite",
  "isInteger",
  "isNaN",
  "isSafeInteger",
  "parseFloat",
  "parseInt",
];

for (const name of names) {
  const value = (Number as unknown as Record<string, unknown>)[name];
  const kind = typeof value;
  console.log(name, kind, kind === "function" ? "<callable>" : String(value));
}

// The callables, exercised rather than merely counted.
console.log(Number.isInteger(3), Number.isInteger(3.5), Number.isSafeInteger(2 ** 53));
console.log(Number.isFinite(Infinity), Number.isNaN(NaN), Number.parseInt("42px"), Number.parseFloat("1.5e2x"));
