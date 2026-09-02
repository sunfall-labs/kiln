// Every static the realm hangs off `Number`, and the exact bytes of each one's value.
//
// THE LIST IS THE REALM'S NOW, NOT THE PROBE'S. It used to be written out here, because the
// native backend refused `Object.getOwnPropertyNames` when the receiver was a builtin constructor
// -- so the probe could only check names somebody had remembered to add to it, which is the same
// disease it was written to catch. ADR 0043 makes both engines answer out of one table, and a
// builtin can describe itself out of that table, so this asks the realm the way
// `global-surface.ts` does: a static added to one engine and not the other changes the output
// without anyone editing this file.
//
// WHY IT EXISTS AT ALL. The interpreter carried fifteen of these; the native prelude carried
// five. `Number.MAX_VALUE` is the first property a bundled `zod` reads, so the two that were
// missing refused the whole module -- and nothing caught it, because no probe asked. Values are
// printed, not just typeof: a static that is present but wrong is a silent wrong answer, which is
// worse than a refusal.
//
// `length` and `name` are dropped rather than printed. They belong to every callable rather than
// to `Number`, and the interpreter reports an arity the native backend does not carry, so
// printing them would make this probe fail for a reason that is not about statics.

const names = Object.getOwnPropertyNames(Number)
  .filter((name) => name !== "length" && name !== "name" && name !== "prototype")
  .sort();

console.log(names.length, names.join(","));

for (const name of names) {
  const value = (Number as unknown as Record<string, unknown>)[name];
  const kind = typeof value;
  console.log(name, kind, kind === "function" ? "<callable>" : String(value));
}

// The callables, exercised rather than merely counted.
console.log(Number.isInteger(3), Number.isInteger(3.5), Number.isSafeInteger(2 ** 53));
console.log(Number.isFinite(Infinity), Number.isNaN(NaN), Number.parseInt("42px"), Number.parseFloat("1.5e2x"));
