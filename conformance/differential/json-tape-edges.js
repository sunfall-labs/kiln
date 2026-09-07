// JSON.parse's reader is one implementation shared by both engines (crates/kiln-json, over the
// simd-json tape). These are the corners where a scanner and the language disagree and the crate
// has to put the language back; each one is also a unit test there, and this pins it against node.
function caught(label, run) {
	try {
		console.log(label, run());
	} catch (error) {
		console.log(label, error.constructor.name, error instanceof SyntaxError);
	}
}

// -0 is negative zero; every other spelling of zero keeps its own sign.
console.log("neg-zero", Object.is(JSON.parse("-0"), -0), Object.is(JSON.parse("0"), 0),
	Object.is(JSON.parse("0.0"), 0), Object.is(JSON.parse("-0.0"), -0), Object.is(JSON.parse("[-0]")[0], -0));

// A number a double cannot hold is Infinity, not a refusal; a spelling JSON forbids still is.
console.log("overflow", JSON.parse("1e400"), JSON.parse("-1e400"), JSON.parse("[1e400,\"1e400\",2]").join("|"),
	JSON.parse('{"1e400":1e-400}')["1e400"]);
caught("overflow-leading-zero", () => JSON.parse("01e400"));
caught("overflow-trailing", () => JSON.parse("1e400x"));

// Integers past 2^53 and past u64 round as any decimal does.
console.log("big-ints", JSON.parse("9007199254740993"), JSON.parse("18446744073709551616"),
	JSON.parse("-9223372036854775809"), JSON.parse("123456789012345678901234567890"));

// Key order is source order, a duplicate keeps its first position and takes its last value, and
// __proto__ is an ordinary own property.
const dup = JSON.parse('{"b":1,"a":2,"b":3,"__proto__":{"x":1},"c":[]}');
console.log("keys", Object.keys(dup).join(","), dup.b, Object.getPrototypeOf(dup) === Object.prototype,
	Object.prototype.hasOwnProperty.call(dup, "__proto__"), dup.__proto__.x);

// Nesting is bounded by the text's length, not by a call depth.
const deep = "[".repeat(3000) + "]".repeat(3000);
let level = 0;
for (let v = JSON.parse(deep); Array.isArray(v); v = v[0]) level += 1;
console.log("deep", level);
const deepObjects = '{"a":'.repeat(2000) + "1" + "}".repeat(2000);
console.log("deep-objects", typeof JSON.parse(deepObjects).a.a.a);

// Empty containers, scalars at the root, and JSON's four whitespace characters.
console.log("shapes", JSON.stringify(JSON.parse(' \t\r\n{"a":[],"b":{},"c":[[]],"d":[{}]} \t\r\n')),
	JSON.parse("null"), JSON.parse(" true "), JSON.parse('"x"'), JSON.parse("[]").length);
caught("empty", () => JSON.parse(""));
caught("space", () => JSON.parse("   "));
caught("two-values", () => JSON.parse("1 2"));
caught("nbsp", () => JSON.parse(" 1"));
caught("bom", () => JSON.parse("﻿1"));
caught("trailing-comma", () => JSON.parse("[1,]"));
caught("single-quotes", () => JSON.parse("{'a':1}"));
caught("literal-tail", () => JSON.parse("[truex]"));
console.log("continued");
