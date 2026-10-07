let calls = 0
const bump = () => { calls += 1; return "key" }
const fallback = () => { calls += 1; return "fallback" }
function run(source, value) {
  return Function("x", "bump", "fallback", source)(value, bump, fallback)
}
console.log(run("return x?.[bump()].deep(bump()) ?? fallback()", null), calls)
console.log(run("return x?.member ?? fallback()", undefined), calls)
console.log(run("return x?.missing.member ?? fallback()", null), calls)
const object = { value: 42, method() { return this.value } }
console.log(run("return x?.method()", object))
console.log(run("return x.method?.()", object))
console.log(run("return x?.method?.()", object))
console.log(run("return x?.value ?? fallback()", object), calls)
console.log(run("return x.missing?.(bump()) ?? fallback()", object), calls)
for (const source of ["return (x?.member).deep", "return x?.missing.deep"]) {
  try { run(source, source.includes("(") ? null : object) }
  catch (error) { console.log(error instanceof TypeError) }
}
console.log(Function("const localOptionalEvalProbe = 1; return eval?.('typeof localOptionalEvalProbe')")())
console.log(Function("return true?.3:0")())
try { console.log(typeof Function("x", "return new x?.method()")) }
catch (error) { console.log(error instanceof SyntaxError) }
