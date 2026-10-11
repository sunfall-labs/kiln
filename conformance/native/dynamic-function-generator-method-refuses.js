// A generator method needs a suspendable frame, which a source-walked dynamic body does not have.
// The native backend must refuse it by name rather than build a method that runs its body eagerly.
// The interpreter has real generators, so this probe is only meaningful as a refusal under --native;
// it prints enough for the interpreter run to be well-formed.
console.log("before");
const o = Function("return { *gen() { yield 1; yield 2; } }")();
console.log("built", typeof o.gen);
console.log("items", JSON.stringify([...o.gen()]));
