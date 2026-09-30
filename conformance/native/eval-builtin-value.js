// Reading eval as a value is distinct from compiling a direct eval call site.
const intrinsicEval = eval;
if (typeof intrinsicEval !== "function") throw new Error("eval is not callable");
if (intrinsicEval.name !== "eval") throw new Error("eval name is not preserved");
console.log("eval builtin value ok");
