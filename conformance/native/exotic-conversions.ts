// Every exotic value kind driven through the *implicit* conversions, rather than through the
// method that names them.
//
// WHY THIS IS A SEPARATE PROBE FROM `global-surface.ts`. That one asks whether a name exists.
// This one asks what a value of each kind *does* when a program uses it without naming a
// conversion at all -- `"total: " + d`, `${d}`, `if (d)`, `JSON.stringify(d)`. Those are the
// spellings real code actually uses, and they reach the runtime through a different door: the
// explicit `d.toString()` is a member lookup, while `${d}` is ToPrimitive and `if (d)` is a tag
// classifier that never looks at a member at all.
//
// The two doors had drifted apart. `d.toString()` answered `2020-03-01` while `String(d)`
// answered `undefined`, `"x" + d` appended nothing, `if (d)` took the *false* branch for a date
// the program was holding, `Object.prototype.toString.call(d)` said `[object Object]`, and
// `JSON.stringify(d)` produced `undefined` at the top level, a missing key inside an object and
// `null` inside an array. Four separate tag classifiers, each written as a list of the kinds it
// knew about, each one kind behind. None of it stopped a program.
//
// Nothing here may throw *uncaught*: the two engines format an uncaught error differently, which
// would report a divergence that is about the reporting rather than about the answer. Where the
// language says to throw -- and `Temporal` says so on purpose, so that `date1 < date2` cannot
// quietly compare two NaNs -- the throw is caught and its name and message are the answer.

const T = Temporal;
function* generator(): Generator<number> { yield 1; }
const weakKey = {};

// The third column is whether the *explicit* door -- `v.toString()` reached as a member -- is
// answered by both engines. It is not, for five kinds: the native backend refuses
// `aMap.toString()` by name, and the interpreter throws "not a function" for a generator, so the
// row would report a refusal rather than a comparison and would stop this probe measuring
// anything after it. That door is a separate gap from the implicit conversions below and is left
// named rather than quietly closed. It is exercised for all eight `Temporal` kinds, which is
// where the two doors had actually drifted apart.
//
// `Uint8Array` is off for a different and worse reason, recorded here so it is not rediscovered:
// the *interpreter* answers `new Uint8Array([1, 2]).toString()` with "\u0001\u0002", because it
// routes `toString` on any `Uint8Array` to node's `Buffer.prototype.toString`, which decodes the
// bytes. node answers "1,2" -- a `Buffer` is a `Uint8Array` and not the other way round -- and so
// does the native backend, which tracks the two apart. So this is a live wrong answer whose
// *native* side is right, and closing it means giving the interpreter the buffer/view distinction
// the backend already has. The implicit conversions below are unaffected and stay on: `String`,
// the template and `join` all answer "1,2" in both engines.
const kinds: [string, unknown, boolean][] = [
  ["PlainDate", new T.PlainDate(2020, 3, 1), true],
  ["PlainTime", new T.PlainTime(1, 2, 3), true],
  ["PlainDateTime", new T.PlainDateTime(2020, 3, 1, 4, 5, 6), true],
  ["PlainYearMonth", new T.PlainYearMonth(2020, 3), true],
  ["PlainMonthDay", new T.PlainMonthDay(3, 1), true],
  ["Duration", new T.Duration(1, 2, 3, 4, 5), true],
  ["Instant", new T.Instant(1_000_000_000n), true],
  ["ZonedDateTime", new T.ZonedDateTime(0n, "UTC"), true],
  ["Map", new Map([[1, 2]]), false],
  ["Set", new Set([1]), false],
  ["WeakMap", new WeakMap([[weakKey, 1]]), false],
  ["WeakSet", new WeakSet([weakKey]), false],
  ["Promise", Promise.resolve(1), false],
  ["Generator", generator(), false],
  ["RegExp", /ab+c/gi, true],
  ["Date", new Date(0), true],
  ["ArrayBuffer", new ArrayBuffer(4), false],
  ["Uint8Array", new Uint8Array([1, 2]), false],
  ["Array", [1, 2], true],
  ["Object", { a: 1 }, true],
];

// Every conversion a program reaches without naming one, plus the two that name it, so a
// disagreement between the two doors shows up as adjacent rows rather than as a missing probe.
const conversions: [string, (v: unknown) => unknown][] = [
  ["String", (v) => String(v), true],
  ["template", (v) => `${v}`, true],
  ["concat", (v) => "x" + (v as string), true],
  ["tag", (v) => Object.prototype.toString.call(v), true],
  ["Number", (v) => Number(v), true],
  ["truthy", (v) => !!v, true],
  ["relational", (v) => (v as number) < (v as number), true],
  ["loose-eq", (v) => (v as number) == (v as number)],
  ["json", (v) => JSON.stringify(v), true],
  ["json-nested", (v) => JSON.stringify({ k: v })],
  ["json-array", (v) => JSON.stringify([v])],
  ["join", (v) => [v].join("|"), true],
  ["method", (v) => (v as { toString(): string }).toString(), true],
];

for (const [name, value, explicit] of kinds) {
  for (const [label, convert] of conversions) {
    if (label === "method" && !explicit) {
      continue;
    }
    let answer: string;
    try {
      answer = JSON.stringify(convert(value)) ?? "undefined";
    } catch (error) {
      answer = `throws ${(error as Error).name}: ${(error as Error).message}`;
    }
    console.log(`${name}\t${label}\t${answer}`);
  }
}
