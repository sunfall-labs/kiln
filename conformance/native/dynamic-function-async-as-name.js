// `async` is only a keyword at the head of an async function; elsewhere it is a plain name.
// uglify-js's parser declares `var async = has_modifier("async", true)`.
const run = (name, source, ...args) => {
  try { console.log(name, JSON.stringify(Function("a", source)(...args))); }
  catch (error) { console.log(name, "threw", error.name); }
};
run("var", "var async = 5; return async;");
run("assign", "var async; async = 5; async++; return async + (async ? 1 : 0);");
run("param", "function g(async) { return async; } return g(3);");
run("fn-value", "var async = function () { return 7; }; return async();");
run("shorthand", "var async = 4; return { async };");
run("property", "var o = { async: 1 }; return o.async + o['async'];");
run("arrow-param", "var f = async => async + 1; return f(1);");
run("async-function", "async function f() { return 1; } return typeof f;");
run("async-expression", "var f = async function () { return 1; }; return typeof f;");
