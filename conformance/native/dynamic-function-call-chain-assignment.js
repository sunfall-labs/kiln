// uglify-js scope.js writes `scope.def_variable(node).defun = defun`: the assigned reference hangs
// off the value of a call.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("call-member", "var o = {}; function f() { return o; } f(1).x = 5; return o.x;");
run("call-member-compound", "var o = { x: 1 }; function f() { return o; } f().x += 5; f().x *= 2; return o.x;");
run("call-computed", "var o = {}; function f() { return o; } f()['k' + 1] = 6; return o.k1;");
run("method-chain", "var o = { inner: { v: 0 }, get() { return this.inner; } }; o.get().v = 8; return o.inner.v;");
run("call-then-index", "var o = [{}]; function f() { return o; } f()[0].x = 5; return o[0].x;");
run("new-member", "function C() { this.v = 1; } var seen; new C().w = 5; return 1;");
run("value-of-assignment", "var o = {}; function f() { return o; } var r = f().x = 4; return [r, o.x];");
run("rhs-evaluated-once", "var n = 0, o = {}; function f() { return o; } f().x = ++n; return [n, o.x];");
run("order", "var log = []; var o = {}; function f() { log.push('base'); return o; } function v() { log.push('rhs'); return 1; } f().x = v(); return log;");
run("target-throws", "function f() { return null; } f().x = 1;");
