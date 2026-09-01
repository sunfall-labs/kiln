// `x instanceof Object` about every shape of value the two engines can both hold.
//
// This is one question asked of many values rather than many questions, and it is here because
// the answer is a *boolean*: a backend that gets it wrong does not refuse and does not crash, it
// says `false` about a value the program is holding in its hand. Nothing else in this corpus
// asked it, and the two engines had drifted apart on a function, a regexp, a promise, a
// generator and every `Temporal` value before anything noticed.
//
// The rule is one line of the specification and has no exceptions to enumerate: every value that
// is not one of the seven primitive types is an Object, so every one of these is `true` and the
// four primitives below are `false`. Written as a table so that a backend which starts listing
// the object kinds one by one -- which is exactly how the drift happened -- fails on the first
// kind it forgets rather than on whichever one a later probe happens to use.
//
// ONE KIND IS DELIBERATELY ABSENT AND SHOULD BE ADDED BACK. A generator object answers `false` in
// the *interpreter*: `prototype_of` has no arm for `Value::Generator`, so its chain is empty and
// every walk of it denies a link -- the same defect `Value::Temporal` had, in the same function,
// one arm away. The native backend answers `true`, which is the correct answer. Listing the
// generator here would therefore not measure equivalence, it would demand that the backend
// reproduce a wrong answer, and agreeing on a wrong answer is the one outcome this corpus exists
// to prevent. The move is the interpreter's: give a generator its chain -- its function's
// `prototype`, then `%GeneratorPrototype%`, then `%IteratorPrototype%` -- and this row goes back.

const values: [string, unknown][] = [
	["object", {}],
	["array", []],
	["function", function () {}],
	["arrow", () => 0],
	["class", class {}],
	["builtin", Object],
	["method", [].map],
	["bound", function () {}.bind(null)],
	["date", new Date(0)],
	["regexp", /x/g],
	["promise", Promise.resolve(1)],
	["map", new Map()],
	["set", new Set()],
	["arraybuffer", new ArrayBuffer(4)],
	["typedarray", new Uint8Array(2)],
	["error", new TypeError("x")],
	// `Temporal.PlainDate.from` is the one `Temporal` construction the native backend carries
	// today -- `new Temporal.X(...)` refuses by name and the other seven `from`s do too -- so it
	// is the one that can be asked here. It is the value whose answer regressed most recently:
	// linking a `Temporal` value to its prototype made the interpreter say `true`, and this side
	// was still saying `false`.
	["temporal", Temporal.PlainDate.from("2020-01-01")],
];

for (const [name, value] of values) {
	console.log(name, value instanceof Object);
}

// The seven primitives, which are the whole of the other side. A `Symbol` and a `BigInt` are
// included on purpose: they are the two newest, and a complement written as a list of exclusions
// is exactly where a newly added primitive gets forgotten.
const primitives: [string, unknown][] = [
	["undefined", undefined],
	["null", null],
	["boolean", true],
	["number", 1],
	["string", "s"],
	["symbol", Symbol("s")],
	["bigint", 1n],
];

for (const [name, value] of primitives) {
	console.log(name, value instanceof Object);
}
