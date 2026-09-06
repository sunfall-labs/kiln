// A tagged template literal must cache its template object canonically per evaluation site
// (ECMA-262 §13.2.8.5 GetTemplateObject).
// Both the cooked template object and its `.raw` property array must be frozen.
// The `.raw` property must be non-enumerable, non-writable, and non-configurable.

function getTemplate(n) {
  return ((strings, ...values) => strings)`hello ${n} world`;
}

const t1 = getTemplate(1);
const t2 = getTemplate(2);

console.log("same-site-equal", t1 === t2);
console.log("same-site-raw-equal", t1.raw === t2.raw);
console.log("frozen-cooked", Object.isFrozen(t1));
console.log("frozen-raw", Object.isFrozen(t1.raw));

const desc = Object.getOwnPropertyDescriptor(t1, "raw");
console.log("raw-descriptor", desc.enumerable, desc.writable, desc.configurable);

// Cooked and raw content
console.log("cooked-elements", t1.length, t1[0], t1[1]);
console.log("raw-elements", t1.raw.length, t1.raw[0], t1.raw[1]);

// Different sites produce distinct template objects
function differentSite() {
  return ((strings) => strings)`hello ${0} world`;
}
const t3 = differentSite();
console.log("different-sites-not-equal", t1 !== t3);
