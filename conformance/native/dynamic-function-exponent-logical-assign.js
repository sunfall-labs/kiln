// `**` (right-associative, with the unary-operand rule) and the logical assignments `&&=`, `||=`
// and `??=` (which only evaluate the right-hand side, and only write, when the left does not
// already decide the answer) inside dynamic code.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("pow", "return 2 ** 10");
run("pow-right-assoc", "return 2 ** 3 ** 2");
run("pow-right-assoc-parens", "return (2 ** 3) ** 2");
run("pow-negative-exponent", "return 2 ** -2");
run("pow-fraction", "return 4 ** 0.5");
run("pow-precedence-mul", "return 2 * 3 ** 2");
run("pow-precedence-mul-right", "return 3 ** 2 * 2");
run("pow-binds-tighter-than-unary-on-right", "return 2 ** -1 ** 1 === undefined");
run("pow-vars", "return a ** b", 3, 4);
run("pow-zero", "return [0 ** 0, NaN ** 0, 0 ** -1]");
run("pow-update-operand", "var i = 2; return ++i ** 2");
run("pow-update-postfix", "var i = 2; return i++ ** 2");
run("pow-member", "var o = { n: 3 }; return o.n ** 2");
run("pow-call", "return Math.abs(-3) ** 2");
run("unary-minus-before-pow", "return -2 ** 2");
run("unary-plus-before-pow", "return +a ** 2", 3);
run("not-before-pow", "return !a ** 2", 3);
run("typeof-before-pow", "return typeof a ** 2", 3);
run("parenthesised-unary-pow", "return (-2) ** 2");
run("pow-assign", "var x = 3; x **= 2; return x");
run("pow-assign-chain", "var x = 2; x **= 3 ** 2; return x");
run("pow-assign-member", "var o = { n: 2 }; o.n **= 5; return o.n");
run("pow-assign-computed", "var o = { n: 2 }; var k = 'n'; o[k] **= 3; return o.n");
run("pow-bigint", "return 2n ** 10n === 1024n");
run("pow-bigint-mixed", "return 2n ** 2");
run("pow-object", "return { valueOf() { return 3; } } ** 2");
run("pow-string-operands", "return '2' ** '3'");

run("or-assign-falsy", "var x = 0; x ||= 7; return x");
run("or-assign-truthy-short-circuits", "var n = 0; var x = 5; x ||= (n++, 9); return [x, n]");
run("and-assign-truthy", "var x = 1; x &&= 7; return x");
run("and-assign-falsy-short-circuits", "var n = 0; var x = 0; x &&= (n++, 9); return [x, n]");
run("nullish-assign-null", "var x = null; x ??= 7; return x");
run("nullish-assign-undefined", "var x; x ??= 7; return x");
run("nullish-assign-zero-kept", "var x = 0; x ??= 7; return x");
run("nullish-assign-empty-kept", "var x = ''; x ??= 7; return x");
run("nullish-assign-false-kept", "var x = false; x ??= 7; return x");
run("nullish-assign-short-circuits", "var n = 0; var x = 1; x ??= (n++, 9); return [x, n]");
run("or-assign-result", "var x = 0; return [x ||= 4, x]");
run("and-assign-result-short", "var x = 0; return [x &&= 4, x]");
run("nullish-assign-result", "var x = null; return [x ??= 'v', x]");
run("assign-member-or", "var o = { k: 0 }; o.k ||= 5; return o");
run("assign-member-or-short", "var o = { k: 1, n: 0 }; o.k ||= (o.n++, 5); return o");
run("assign-member-and", "var o = { k: 2 }; o.k &&= 'set'; return o");
run("assign-member-nullish-new-key", "var o = {}; o.k ??= 'made'; return o");
run("assign-computed-nullish", "var o = { a: null }; var k = 'a'; o[k] ??= 3; return o");
run("assign-getter-read-once", "var reads = 0; var o = { get v() { reads++; return 1; } }; o.v ||= 2; return reads");
run("assign-setter-not-called-when-short", "var writes = 0; var o = { get v() { return 1; }, set v(x) { writes++; } }; o.v ||= 2; return writes");
run("assign-setter-called", "var writes = 0; var o = { get v() { return 0; }, set v(x) { writes++; } }; o.v ||= 2; return writes");
run("assign-call-chain", "var o = { get() { return this.t; }, t: { v: null } }; o.get().v ??= 4; return o.t");
run("assign-param", "a ??= 'dflt'; return a", undefined);
run("assign-param-kept", "a ??= 'dflt'; return a", 'given');
run("assign-const-short-circuit", "const c = 1; c ||= 2; return c");
run("assign-const-write-throws", "const c = 0; c ||= 2; return c");
run("assign-undeclared-strict", "'use strict'; undeclared ??= 1; return 1");
run("assign-undeclared-sloppy-write", "undeclaredLogicalTarget ??= 1; return typeof undeclaredLogicalTarget");
run("assign-chain-right-assoc", "var x = null, y = null; x ??= y ??= 3; return [x, y]");
run("assign-in-condition", "var x = 0; if (x ||= 2) return x; return -1");
run("assign-rhs-function", "var f; f ??= function () { return 'made'; }; return f()");
run("assign-rhs-arrow", "var f; f ||= () => 'arrow'; return f()");
run("assign-rhs-template", "var s = ''; s ||= `t${a}`; return s", 4);
run("assign-lhs-and-rhs-regex", "var r = null; r ??= /x\\/y/; return r.source");
run("assign-bitwise-still-works", "var x = 6; x &= 3; x |= 8; x ^= 1; return x");
run("logical-operators-still-work", "return [0 || 'a', 1 && 'b', null ?? 'c', 0 ?? 'd']");

console.log("eval-pow", (0, eval)("2 ** 3 ** 2"));
console.log("eval-nullish", (0, eval)("(() => { var q; q ??= 5; return q; })()"));
