// Numbers as the program sees them: the global conversion functions, the methods a number
// primitive answers to, and the names a console gives the callables the runtime supplies.
//
// The last of those is here because it is the part that fails silently. A wrong `parseInt` shows
// up as a wrong number; a wrong name shows up only if something prints one, and until this probe
// the native backend called every builtin `String`.

// `Boolean(x)` is `!!x` spelled as a call, and no argument is false rather than undefined.
console.log(Boolean(""), Boolean("0"), Boolean([]), Boolean({}), Boolean(), Boolean(0), Boolean(NaN));

// The two tolerant parsers read as far as they can and stop, which is their whole difference
// from `Number(x)`.
console.log(parseInt("12px"), parseInt("0x1f"), parseInt("ff", 16), parseInt("-42"), parseInt("x"));
console.log(parseInt("  7 "), parseInt("12", 2), parseInt(""), parseInt("+8"), parseInt("0X10"));
console.log(parseFloat("1.5e3abc"), parseFloat(".5"), parseFloat("-Infinity"), parseFloat("x"));
console.log(parseFloat("1e"), parseFloat("0x1f"), parseFloat("  2.5  "), parseFloat("-.25e-2"));

// A radix outside 2..36 is a RangeError, and so is a digit count outside 0..100 -- returning a
// number would look like a result.
try { (1).toString(99); } catch (error: any) { console.log("radix", error.constructor.name); }
try { (1).toFixed(101); } catch (error: any) { console.log("digits", error.constructor.name); }

// The global pair coerces and the `Number` pair does not, which is why both exist.
console.log(isNaN("x"), Number.isNaN("x"), isFinite("1"), Number.isFinite("1"));
console.log(Number.parseInt("9px"), Number.parseFloat("2.5em"), Number("12"), Number(""), Number());

// Read off the value: there is no `Number.prototype` object on either side yet.
console.log((255).toString(16), (255).toString(2), (0.5).toString(2), (-10.25).toString(4));
console.log((0).toString(8), (1.005).toFixed(2), (1234.5678).toFixed(3), (0).toFixed(2));
console.log((1 / 0).toFixed(2), (42).valueOf(), (3.7).toString(), (-0).toString());

// A runtime-supplied callable is named for the program's spelling of it, not the implementation's.
console.log(String, Number, Boolean, Object, Array, Promise, Map, Set, Date);
// `Error` itself is left out: node prints its own `stackTraceLimit` property alongside the
// name, which is node's rather than the language's.
console.log(parseInt, parseFloat, isNaN, isFinite, TypeError, RangeError, SyntaxError);
// A namespace is an object holding functions, and prints and converts as one.
console.log(Math, JSON, typeof Math, typeof JSON, String(Math), `${JSON}`);
// `String(f)` of a runtime-supplied function is node's source text, which no backend has. Both
// kiln engines name the function instead, so this checks it is named rather than what it says.
console.log(`${parseInt}`.length > 0, String(Boolean).length > 0, typeof Boolean, typeof parseInt);

// `Math`'s second half, and the twelve functions neither engine had. Two of them exist precisely
// because the two-step spelling is wrong: `log1p` and `expm1` keep the significant digits that
// `log(1 + x)` and `exp(x) - 1` lose for small `x`.
console.log(Math.sinh(0), Math.cosh(0), Math.tanh(0), Math.asinh(0), Math.acosh(1), Math.atanh(0));
console.log(Math.log1p(0), Math.expm1(0), Math.log1p(1e-10), Math.expm1(1e-10));
console.log(Math.sinh(1).toFixed(6), Math.tanh(1).toFixed(6), Math.acosh(2).toFixed(6));
// `fround` is a round trip through `float`, not a truncation; `imul` truncates *first*, which is
// what makes it different from multiplying.
console.log(Math.fround(1.1), Math.fround(5.5), Math.clz32(1), Math.clz32(0), Math.clz32(-1));
console.log(Math.imul(3, 4), Math.imul(-5, 12), Math.imul(0xffffffff, 5), Math.imul(1e9, 1e9));

// `Math.random` is seeded and reproducible (ADR 0026), which is why the *values* can be checked
// here rather than only their range: both engines run the same generator from the same seed, so a
// disagreement is a bug rather than the weather. Node is not asked -- it could not agree.
const drawn: number[] = [];
for (let index = 0; index < 5; index += 1) { drawn.push(Math.random()); }
console.log(drawn.every((value) => value >= 0 && value < 1), new Set(drawn).size, typeof Math.random());
