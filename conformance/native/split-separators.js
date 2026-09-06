// `String.prototype.split` on ASCII text with an ASCII separator, the case the native backend
// now fast-paths (strings.c, `KILN_C99_METHOD_SPLIT`): a separator absent entirely, at the very
// start, at the very end, two adjacent separators (an empty part between them), an empty
// receiver, an empty separator (splits into code units), and a separator longer than one byte.
console.log("abc".split("x"));
console.log(" abc".split(" "));
console.log("abc ".split(" "));
console.log("a  b".split(" "));
console.log("".split(" "));
console.log("abc".split(""));
console.log("a::b::c".split("::"));
console.log("::".split("::"));
console.log(":a:".split(":"));
console.log("a".split("a"));
