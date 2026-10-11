// eta's compiled templates define `async function blockAsync` next to the sync one even when only
// the sync one runs, so the async body has to parse; `await` as an ordinary name must survive.
const compiled = Function("fn", [
  "async function pick(x) { return x ? await fn(x) : ''; }",
  "async function twice(x) { const r = await fn(x); return await (r + r); }",
  "return typeof pick + typeof twice;",
].join("\n"));
console.log(compiled(() => 1));
const named = Function("var await = 5; return await + 1;");
console.log(named());
const called = Function("function await(x) { return x * 2; } return await(21);");
console.log(called());
