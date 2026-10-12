// zod's generated validators write `["name", ...iss.path]`; arktype's write `keys.push(...symbols)`.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", "b", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("array-spread", "return [0, ...a, 9]", [1, 2]);
run("array-spread-ternary", "return a ? [...a, 'x'] : ['name']", [7]);
run("array-spread-twice", "return [...a, ...a]", "ab");
run("array-spread-set", "return [...new Set(a)]", [1, 1, 2]);
run("array-hole", "var h = [1, , 3]; return [h.length, 1 in h, 2 in h]");
run("array-trailing-comma", "return [1, 2, ].length");
run("call-spread", "var k = [1]; k.push(...a); return k", [2, 3]);
run("call-spread-call", "var k = []; k.push(...Object.keys(a)); return k", { x: 1, y: 2 });
run("call-spread-mixed", "return Math.max(0, ...a, 10, ...b)", [3, 4], [20]);
run("call-spread-empty", "var k = [5]; k.push(...a); return k", []);
run("call-spread-string", "return String.fromCharCode(...a)", [104, 105]);
run("call-spread-many", "return Math.max(...a)", Array.from({ length: 40 }, (_, i) => i));
run("new-spread", "return new Date(...a).getUTCFullYear()", [2020, 1, 2]);
run("method-spread", "var o = { m(x, y, z) { return x + y + z; } }; return o.m(...a)", [1, 2, 3]);
run("spread-not-iterable", "return [...a]", 5);
