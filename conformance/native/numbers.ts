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
