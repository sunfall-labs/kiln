// Issue 25: ClassDefinitionEvaluation refuses a heritage that is neither `null` nor a
// constructor, and one whose `prototype` is neither an object nor `null`, with a `TypeError`
// before the class exists. The native backend defined all of them; the interpreter accepted a
// constructor with a primitive `prototype` and threw for `extends null`, which is a class whose
// prototype object inherits from nothing and whose constructor still inherits from
// `Function.prototype`. Recorded from node 26.8.1.
function h(label, make) {
  try { make(); console.log(label, "no throw"); }
  catch (e) { console.log(label, e.constructor.name); }
}
h("number", () => { const n = 1; class U extends n {} });
h("string", () => { const n = "s"; class U extends n {} });
h("undefined", () => { const n = undefined; class U extends n {} });
h("symbol", () => { const n = Symbol(); class U extends n {} });
h("plain object", () => { const n = {}; class U extends n {} });
h("arrow", () => { const n = () => {}; class U extends n {} });
h("arrow with a prototype", () => { const n = () => {}; n.prototype = {}; class U extends n {} });
h("async", () => { const n = async function () {}; class U extends n {} });
h("generator", () => { const n = function* () {}; class U extends n {} });
h("async generator", () => { const n = async function* () {}; class U extends n {} });
h("method", () => { const n = { m() {} }.m; class U extends n {} });
h("prototype 3", () => { function F() {} F.prototype = 3; class U extends F {} });
h("prototype string", () => { function F() {} F.prototype = "p"; class U extends F {} });
h("class expression", () => { const U = class extends (1) {}; });
h("function", () => { function F() {} class U extends F {} });
h("class", () => { class A {} class U extends A {} });
h("builtin", () => { class U extends Array {} });
h("prototype null", () => { function F() {} F.prototype = null; class U extends F {} });
h("prototype function", () => { function F() {} F.prototype = function () {}; class U extends F {} });
class N extends null {}
console.log(Object.getPrototypeOf(N) === Function.prototype, Object.getPrototypeOf(N.prototype));
function P() {} P.prototype = null;
class Q extends P {}
console.log(Object.getPrototypeOf(Q) === P, Object.getPrototypeOf(Q.prototype));
class R extends Array {}
const r = new R(); r.push(1, 2);
console.log(r.length, r instanceof Array, Object.getPrototypeOf(R) === Array);
