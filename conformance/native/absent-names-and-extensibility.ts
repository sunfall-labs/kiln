// Two things a program does to an object it did not build: read a name that is not there, and ask
// whether it may add one.
//
// AN ABSENT NAME IS `undefined`, NOT A REFUSAL. `a["nope"]` on an array and `d["nope"]` on a date
// are not gaps in the native backend -- they are what the language answers. Refusing them ended
// the process instead. A bundled `vue-reactivity` reads arbitrary names off arrays because that
// is what a reactive proxy does, and `dayjs` does the same to dates; both stopped there, and
// `dayjs` runs natively now because this was fixed.
//
// The other half is why this probe lists real member names too: a member the backend has not
// implemented must still refuse *by name*. Answering undefined for one would break
// `typeof x.member === "function"`, which is the silent kind of wrong -- the program takes the
// other branch and nothing reports anything.
//
// EXTENSIBILITY IS WEAKER THAN FREEZING, and the difference is the whole point of asking. A
// non-extensible object still takes writes to properties it already has. `Object.isFrozen` on one
// is false. Answering the freeze flag for `isExtensible` would have been a wrong answer rather
// than a missing one.

const array: Record<string, unknown> = [1, 2] as never;
const date: Record<string, unknown> = new Date(0) as never;
for (const name of ["nope", "0", "length", "map", "constructor", "getTime", "toISOString"]) {
  console.log("array", name, typeof array[name]);
  console.log("date", name, typeof date[name]);
}

const target: Record<string, unknown> = { kept: 1 };
console.log("fresh", Object.isExtensible(target), Object.isFrozen(target));
Object.preventExtensions(target);
console.log("prevented", Object.isExtensible(target), Object.isFrozen(target));
target.kept = 2;
console.log("existing-write", target.kept);
try {
  target.added = 3;
  console.log("add", "no throw", target.added);
} catch (error) {
  console.log("add", (error as Error).constructor.name, (error as Error).message);
}

// Freezing implies non-extensible even though nothing asked for it.
console.log("frozen", Object.isExtensible(Object.freeze({ a: 1 })));
// A primitive is not an error: the specification reports it as non-extensible.
console.log("primitive", Object.isExtensible(1 as never), Object.isExtensible("s" as never));
