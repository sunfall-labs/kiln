// `new.target` and `async` arrow functions inside dynamic code.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("new-target-call", "function F() { return new.target === undefined; } return F()");
run("new-target-construct", "function F() { this.same = new.target === F; } return new F().same");
run("new-target-typeof", "function F() { return typeof new.target; } return [F(), typeof new F()]");
run("new-target-guard", "function F() { if (!new.target) return new F(); this.made = true; } return F().made");
run("new-target-in-arrow", "function F() { var g = () => new.target; this.same = g() === F; } return new F().same");
run("new-target-arrow-call", "function F() { var g = () => new.target; return g() === undefined; } return F()");
run("new-target-nested-function", "function F() { function G() { return new.target; } return G() === undefined; } return new F() instanceof F");
run("new-target-reflect-construct", "function F() { return new.target; } function Other() {} return Reflect.construct(F, [], Other) === Other");
run("new-target-property", "function F() { this.same = new.target.prototype === F.prototype; } return new F().same");
run("new-target-after-nested-call", "function G() {} function F() { G(); return new.target === F; } return new F() instanceof F");
run("new-target-method", "var o = { m() { return new.target; } }; return o.m() === undefined");
run("new-target-function-constructor", "var F = Function('this.t = new.target !== undefined'); return [new F().t, typeof F()]");
run("new-target-misspelled", "function F() { return new.targe; } return F()");
run("new-expression-still-works", "function P(v) { this.v = v; } return new P(3).v");
run("new-member-still-works", "var ns = { P: function (v) { this.v = v; } }; return new ns.P(4).v");
run("new-no-args-still-works", "function P() { this.v = 5; } return new P().v");

const asyncRun = async (name, source, ...args) => {
  try { console.log(name, JSON.stringify(await Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
await asyncRun("async-arrow-paren", "var f = async (x) => x + 1; return f(1)", 0);
await asyncRun("async-arrow-name", "var f = async x => x * 2; return f(4)");
await asyncRun("async-arrow-no-params", "var f = async () => 'none'; return f()");
await asyncRun("async-arrow-multi", "var f = async (x, y) => x + y; return f(1, 2)");
await asyncRun("async-arrow-block", "var f = async (x) => { return x * 3; }; return f(3)");
await asyncRun("async-arrow-default", "var f = async (x = 7) => x; return f()");
await asyncRun("async-arrow-destructured", "var f = async ({ k }, [v]) => k + v; return f({ k: 'a' }, ['b'])");
await asyncRun("async-arrow-rest", "var f = async (...r) => r.length; return f(1, 2, 3)");
await asyncRun("async-arrow-returns-promise", "var f = async () => 1; return f() instanceof Promise");
await asyncRun("async-arrow-typeof", "var f = async (x) => x; return typeof f");
await asyncRun("async-arrow-no-prototype", "var f = async (x) => x; return f.hasOwnProperty('prototype')");
await asyncRun("async-arrow-lexical-this", "var o = { v: 9, m() { var f = async () => this.v; return f(); } }; return o.m()");
await asyncRun("async-arrow-throws-rejects", "var f = async () => { throw new RangeError('x'); }; return f().catch((e) => e.name)");
await asyncRun("async-arrow-immediately-invoked", "return (async (x) => x + 1)(1)");
await asyncRun("async-arrow-as-argument", "var run = (f) => f(5); return run(async (n) => n * 2)");
await asyncRun("async-arrow-in-object", "var o = { f: async (x) => x, g: async y => y }; return Promise.all([o.f(1), o.g(2)])");
await asyncRun("async-arrow-in-array", "var fs = [async () => 1, async () => 2]; return Promise.all(fs.map((f) => f()))");
await asyncRun("async-arrow-capture", "var base = 10; var f = async (x) => x + base; return f(1)");
await asyncRun("async-arrow-ternary", "var f = a ? async () => 'yes' : async () => 'no'; return f()", true);
await asyncRun("async-as-identifier-call", "var async = (x) => x + 1; return async(1)");
await asyncRun("async-as-identifier-arrow", "var async = 5; var f = (x) => async + x; return f(1)");
await asyncRun("async-as-identifier-plain-arrow", "var f = async => async; return f(2)");
await asyncRun("async-function-expression-still-works", "var f = async function (x) { return x; }; return f(6)");
await asyncRun("async-function-declaration-still-works", "async function f(x) { return x + 1; } return f(6)");
