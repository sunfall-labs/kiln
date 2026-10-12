// Statement heads, conditions and return operands take a full Expression, comma included.
// uglify-js builds its whole library through new Function and writes `if (c) a, next(); else b`.
const run = (name, ...parts) => {
  try { console.log(name, JSON.stringify(Function("a", "b", parts.join("\n"))(1, 2))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("statement", "var x; x = 1, x = 2; return x");
run("if-then", "var i = 0; function next() { i = 9; }", "if (a) a, next();", "else i = 3;", "return i");
run("if-else", "var i = 0; function next() { i = 9; }", "if (!a) a, next();", "else i = 3;", "return i");
run("for-update", "var s = 0; for (var i = 0, j = 5; i < 2; i++, j--) s += j; return s");
run("while-body", "var n = 0; while (n < 3) n++, n++; return n");
run("return", "return a, b");
run("throw", "try { throw a, b } catch (e) { return e }");
run("condition", "var n = 0; if (n++, n > 0) return 'seq'; return 'no'");
console.log("eval", eval("1, 2, 3"), (0, eval)("4, 5"));
