// `error.stack` names file, line and column for every frame, module level included.
function a() { return b(); }
function b() { return new Error("where"); }
const e = a();
console.log(e.stack);
const m = { go() { return new TypeError("method"); } };
console.log(m.go().stack);
console.log(new Error("top level").stack);
