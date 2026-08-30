/*---
description: Kiln smoke test for eval's three forms and the scopes each of them sees
flags: [raw]
features: [kiln-smoke, eval]
---*/
// `raw`, so this brings its own reporting: the assertion harness is Test262's and this corpus is
// Kiln's own. Every mismatch is collected rather than thrown at, so one failure names all of them.
var wrong = [];
function is(actual, expected, label) {
  if (actual !== expected) {
    wrong.push(label + ": got " + String(actual) + ", wanted " + String(expected));
  }
}

// A direct eval evaluates in the caller's scope, which is the whole of what makes it direct.
function reads() { var x = 41; return eval("x + 1"); }
is(reads(), 42, "a direct eval reads a caller local");

function writes() { var x = 1; eval("x = 2"); return x; }
is(writes(), 2, "a direct eval writes through to the caller's binding");

function shadows() { var Array = 7; return eval("Array"); }
is(shadows(), 7, "a caller's binding shadows the built-in of the same name");

function throughAClosure() {
  var captured = 3;
  function inner() { return eval("captured * 2"); }
  return inner();
}
is(throughAClosure(), 6, "a direct eval reaches a binding the caller captured");

function declaresItsOwn() { return eval("var made = 5; made"); }
is(declaresItsOwn(), 5, "eval code can declare and read bindings of its own");

// Every other route to the same function is an *indirect* eval: global scope, no caller locals.
function indirect() { var hidden = 1; return typeof (0, eval)("typeof hidden"); }
is(indirect(), "string", "an indirect eval still evaluates its source");
is((0, eval)("2 + 3"), 5, "an indirect eval through a comma expression");
is(globalThis.eval("4 * 5"), 20, "an indirect eval read off the global object");

// `eval` is an ordinary function value with the identity the specification gives it.
is(typeof eval, "function", "eval is callable");
is(eval.name, "eval", "eval's name");
is(eval.length, 1, "eval's declared arity");

// Anything but a string comes back unchanged rather than being coerced and run.
is(eval(7), 7, "eval of a non-string is that value");

// The completion value of the source, which is what makes eval an expression. The string is not
// the first statement on purpose: a leading string literal is a *directive*, and the frontend
// drops directives rather than lowering them, so `eval("'ok'")` still answers undefined.
is(eval("1; 'ok'"), "ok", "eval answers with its completion value");
is(eval(""), undefined, "empty source completes with undefined");

// Source that is not a program is a SyntaxError the caller can catch.
var caught = null;
try { eval("var 1 ="); } catch (error) { caught = error; }
is(caught === null ? "no throw" : caught.constructor.name, "SyntaxError", "bad source");

// `Function` builds from source too, and always in global scope.
is(new Function("a", "b", "return a + b")(2, 3), 5, "new Function from source");
is(Function("return 7")(), 7, "Function called without new");
is(typeof new Function(""), "function", "an empty body still builds a function");

if (wrong.length !== 0) {
  throw new Error("eval smoke: " + wrong.join("; "));
}
