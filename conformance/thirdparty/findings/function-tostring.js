// `Function.prototype.toString` answers a placeholder rather than source text.
//
// A plausible string rather than an error, so a library that parses it -- a dependency injector
// reading parameter names, or the `/native code/` test every polyfill uses to decide whether an
// intrinsic is real -- gets a confident wrong answer.
//
// node:  "function f(a) { /* body */ return a + 1; }" / "(x) => x * 2" /
//        "function max() { [native code] }" / "class C { [k]() { re"
// kiln:  "[Function 0]" / "[Function 3]" / "[Function: max]" / "[Function 1]"
const k = Symbol("k");
function f(a) { /* body */ return a + 1; }
class C { [k]() { return "m"; } }
console.log("fn-toString", JSON.stringify(f.toString()));
console.log("arrow-toString", JSON.stringify(((x) => x * 2).toString()));
console.log("native-toString", JSON.stringify(Math.max.toString()));
console.log("class-toString", JSON.stringify(C.toString().slice(0, 20)));
