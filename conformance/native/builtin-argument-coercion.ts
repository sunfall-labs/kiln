// A built-in's arguments, converted the way the language says -- for every built-in that takes
// one, rather than for the three that were noticed.
//
// WHY THIS PROBE IS A WALK AND NOT A LIST OF ASSERTIONS. `ToNumber` of an object is the
// program's own `valueOf`: a *call*, which can be counted, can throw, and can see what has
// already happened. A pure conversion table cannot make that call, so it answers NaN and moves
// on -- and every built-in behind it then computes confidently from a number the program never
// supplied. That failure is silent by construction: `[1,2,3,4].slice({valueOf: () => 1})`
// returns an array, `(5.5).toFixed({valueOf: () => 2})` returns a string, and neither looks
// wrong until it is compared with the other engine. The interpreter grew the pass in `a7c65ee7`
// and `41817525`; the C backend did not follow, and nothing here noticed for as long as the
// corpus asked about three methods instead of about the rule.
//
// So the first section walks every position in both engines' coercion tables at once. Adding a
// built-in to either table without adding it here is the mistake this is shaped to catch: the
// walk prints one line per entry, so a built-in that stops converting shows up as a changed line
// rather than as a missing one.
//
// The second section is the other half of the same rule, and the half that made the first half
// invisible: a numeric argument is no use if the built-in then ignores it. Every search method
// took a position and discarded it, so the coercion could be fixed in full and
// `"abcabc".indexOf("c", 3)` would still answer 2.

// A subject that records what was asked of it. Both conversions are defined so that the *choice*
// is visible: a built-in reading text must not reach `valueOf`, and one reading a number must not
// reach `toString`.
function counted(numeric: number, text: string) {
  const seen: string[] = [];
  return {
    seen,
    subject: {
      valueOf() { seen.push("valueOf"); return numeric; },
      toString() { seen.push("toString"); return text; },
    },
  };
}

function walk(label: string, call: (subject: any) => any) {
  const { seen, subject } = counted(1, "1");
  let answer: string;
  try {
    answer = String(call(subject));
  } catch (error: any) {
    answer = "threw " + error.constructor.name;
  }
  // The conversions are part of the answer, not commentary on it: a built-in that produces the
  // right string without calling `valueOf` got there by guessing.
  console.log(label, answer, "[" + seen.join(",") + "]");
}

// Numbers. `toString`'s radix, and the three digit-count renderings -- two of which the native
// backend did not carry at all and answered with a TypeError from calling `undefined`.
walk("number.toString  ", (o) => (255).toString({ valueOf() { return 16; } } as any));
walk("number.toFixed   ", (o) => (5.5).toFixed(o));
walk("number.toExponent", (o) => (1234.5678).toExponential(o));
walk("number.toPrecisio", (o) => (1234.5678).toPrecision(o));

// Strings, by position. Every one of these is `ToIntegerOrInfinity`, which is a `ToNumber` first.
walk("string.slice     ", (o) => "abcdef".slice(o));
walk("string.substring ", (o) => "abcdef".substring(o));
walk("string.substr    ", (o) => "abcdef".substr(o));
walk("string.at        ", (o) => "abcdef".at(o));
walk("string.charAt    ", (o) => "abcdef".charAt(o));
walk("string.charCodeAt", (o) => "abcdef".charCodeAt(o));
walk("string.codePointA", (o) => "abcdef".codePointAt(o));
walk("string.indexOf   ", (o) => "abcabc".indexOf("c", o));
walk("string.lastIndexO", (o) => "abcabc".lastIndexOf("c", o));
walk("string.includes  ", (o) => "abcabc".includes("c", o));
walk("string.startsWith", (o) => "abcabc".startsWith("b", o));
walk("string.endsWith  ", (o) => "abcabc".endsWith("a", o));
walk("string.padStart  ", (o) => "ab".padStart(o));
walk("string.padEnd    ", (o) => "ab".padEnd(o));
walk("string.repeat    ", (o) => "ab".repeat(o));

// Arrays. `fill`'s *value* and `indexOf`'s *needle* are deliberately not converted -- they are
// any value at all -- which is why each of those is passed the subject in the position that is
// a number and a plain value in the position that is not.
walk("array.slice      ", (o) => [1, 2, 3, 4].slice(o).join(","));
walk("array.splice     ", (o) => [1, 2, 3, 4].splice(o).join(","));
walk("array.at         ", (o) => [1, 2, 3, 4].at(o));
walk("array.fill       ", (o) => [1, 2, 3, 4].fill(9, o).join(","));
walk("array.copyWithin ", (o) => [1, 2, 3, 4].copyWithin(o).join(","));
walk("array.indexOf    ", (o) => [1, 2, 3, 4].indexOf(3, o));
walk("array.lastIndexOf", (o) => [1, 2, 3, 4].lastIndexOf(3, o));
walk("array.includes   ", (o) => [1, 2, 3, 4].includes(3, o));

// Text arguments, which take the other branch of the same pass: `ToString`, so `toString` runs
// and `valueOf` does not.
walk("escape           ", (o) => escape(o));
walk("unescape         ", (o) => unescape(o));

// The globals that coerce, beside the `Number` pair that deliberately does not -- the whole
// reason both spellings exist.
walk("isNaN            ", (o) => isNaN(o));
walk("isFinite         ", (o) => isFinite(o));
walk("Number.isNaN     ", (o) => Number.isNaN(o));
walk("Number.isInteger ", (o) => Number.isInteger(o));
walk("Math.floor       ", (o) => Math.floor(o));
walk("Math.pow         ", (o) => Math.pow(o, 2));
walk("parseInt         ", (o) => parseInt(o));
walk("parseFloat       ", (o) => parseFloat(o));

// The conversion is a call, so it can throw, and the throw is the program's own.
const raised = { valueOf() { throw new RangeError("from valueOf"); } };
try { "abcdef".slice(raised as any); } catch (error: any) {
  console.log("throwing valueOf", error.constructor.name, error.message);
}
try { "ab".repeat(raised as any); } catch (error: any) {
  console.log("throwing valueOf", error.constructor.name, error.message);
}

// Order is observable, so it is part of the contract rather than an implementation detail: two
// arguments that both convert must convert left to right, and two that both throw must throw the
// leftmost first.
//
// The receiver's own conversion is the other half of this rule and is not asked here, because
// the only way to reach it -- `String.prototype.slice.call(object, ...)`, a generic method on a
// receiver that is not a string -- is refused by the native backend today. Asking anyway would
// refuse the probe at this line and silently stop everything below it from measuring, which is
// the trap `conformance/native/temporal.ts` documents and then fell into.
const order: string[] = [];
const left = { valueOf() { order.push("left"); return 1; } };
const right = { valueOf() { order.push("right"); return 3; } };
console.log("abcdef".slice(left as any, right as any), order.join(","));
const raisedLeft = { valueOf() { order.push("raised-left"); throw new RangeError("left"); } };
const raisedRight = { valueOf() { order.push("raised-right"); throw new RangeError("right"); } };
try { "abcdef".slice(raisedLeft as any, raisedRight as any); } catch (error: any) {
  console.log("leftmost throws first", error.message, order.join(","));
}

// `ToNumber` is the step that refuses a symbol and a BigInt, and refusing is an answer where a
// silent NaN is not.
try { "abcdef".slice(Symbol("s") as any); } catch (error: any) {
  console.log("symbol argument", error.constructor.name);
}
try { "abcdef".slice(10n as any); } catch (error: any) {
  console.log("bigint argument", error.constructor.name);
}

// `undefined` is left alone rather than converted, because it is a *meaningful* argument and
// each built-in says for itself what it means: `slice(1, undefined)` runs to the end, where
// `ToNumber(undefined)` would be NaN and yield nothing.
console.log("abcdef".slice(1, undefined), [1, 2, 3].slice(1, undefined).join(","));
console.log((1.5).toFixed(undefined), (1.5).toPrecision(undefined), (1.5).toExponential(undefined));

// NaN reaches these as an ordinary argument once the conversion runs, and every one of them has
// to answer rather than trap. `substr` and `codePointAt` cast their position straight to an
// unsigned index, and a NaN has no value to cast: on an aborting target that ended the process
// instead of printing anything, which is the one failure a comparison of two outputs cannot
// describe.
console.log("abcdef".substr(NaN), "abcdef".codePointAt(NaN), "abcdef".charCodeAt(NaN));
console.log("abcdef".slice(NaN), "abcdef".at(NaN), [1, 2, 3].at(NaN), "abcdef".substring(NaN));
console.log("abcdef".indexOf("c", NaN), "abcabc".lastIndexOf("c", NaN), [1, 2].indexOf(2, NaN));

// The other half of the rule: a position that arrives correctly and is then ignored is the same
// wrong answer by a different route. Each of these was answered from a search that started at
// zero regardless of what was asked.
console.log("abcabc".indexOf("c", 3), "abcabc".indexOf("c", 9), "abcabc".indexOf("c", -1));
console.log("abcabc".lastIndexOf("c", 1), "abcabc".lastIndexOf("c", 4), "abcabc".lastIndexOf("c"));
console.log("abcabc".includes("a", 1), "abcabc".includes("a", 4), "abcabc".includes("a"));
console.log("abcabc".startsWith("b", 1), "abcabc".startsWith("b", 9), "abcabc".startsWith("a", -1));
console.log("abcabc".endsWith("b", 2), "abcabc".endsWith("c", 0), "abcabc".endsWith("c"));
console.log([1, 2, 3, 4].indexOf(3, 3), [1, 2, 3, 4].indexOf(3, -2), [1, 2, 3, 4].indexOf(3, -9));
console.log([1, 2, 3, 4].lastIndexOf(3, 1), [1, 2, 3, 4].lastIndexOf(3, -1));
console.log([1, 2, 3, 4].lastIndexOf(3, -3), [1, 2, 3, 4].lastIndexOf(3, -9), [3].lastIndexOf(3, -9));
console.log([1, 2, 3, 4].includes(3, 3), [1, 2, 3, 4].includes(3, -2), [1, 2, 3, 4].includes(3, 9));

// The three renderings the native backend had to learn, across the ranges where each changes
// shape: `toPrecision` switches to the exponential form at its own window, and `toExponential`
// with no argument asks for the fewest digits that name the value exactly.
console.log((1234.5678).toExponential(2), (1234.5678).toExponential(0), (0).toExponential(2));
console.log((1234.5678).toExponential(), (100).toExponential(), (0.000001).toExponential());
console.log((-1234.5678).toExponential(3), (1e21).toExponential(2), (1e-7).toExponential(1));
console.log((1234.5678).toPrecision(6), (1234.5678).toPrecision(2), (1234.5678).toPrecision(10));
console.log((0.0000001).toPrecision(2), (123).toPrecision(3), (0).toPrecision(3));
console.log((-0.5).toPrecision(1), (999.9).toPrecision(3), (9.99).toPrecision(2));
console.log((1 / 0).toExponential(2), (-1 / 0).toPrecision(3), (0 / 0).toExponential(2));

// A digit count outside the range each accepts is a RangeError, and the ranges differ: zero
// digits is a whole number for `toExponential` and no number at all for `toPrecision`.
try { (1).toExponential(101); } catch (error: any) { console.log("toExponential", error.constructor.name); }
try { (1).toPrecision(0); } catch (error: any) { console.log("toPrecision", error.constructor.name); }
try { (1).toPrecision(101); } catch (error: any) { console.log("toPrecision", error.constructor.name); }
// ...but an infinite receiver is answered from `ToString` *after* the range check, not before.
console.log((1 / 0).toExponential(101 - 101), (1 / 0).toPrecision(1));
