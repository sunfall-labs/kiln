// What `require("node:querystring")` publishes, and what functions answer.
const qs = require("node:querystring");

const names = Object.getOwnPropertyNames(qs).sort();
console.log(names.length);
for (const name of names) {
  console.log(name, typeof qs[name]);
}

console.log("parse === decode", qs.parse === qs.decode);
console.log("stringify === encode", qs.stringify === qs.encode);
console.log("default self-link", qs.default === qs);

// Form parsing with repeats, percent-encoding, spaces
const query = qs.parse("alpha=1&beta=2&alpha=3&gamma=foo+bar&delta=%E2%9C%93&__proto__=safe");
console.log("null prototype", Object.getPrototypeOf(query) === null);
console.log("query.alpha", query.alpha.join(","));
console.log("query.beta", query.beta);
console.log("query.gamma", query.gamma);
console.log("query.delta", query.delta);
console.log("query.__proto__", query.__proto__);

// Stringifying
const str = qs.stringify({ x: "hello world", y: [1, 2] });
console.log("str", str);

// Escape and unescape
const esc = qs.escape("a/b?c=d&e=f");
const unesc = qs.unescape("a%2Fb%3Fc%3Dd%26e%3Df");
console.log("esc", esc);
console.log("unesc", unesc);
