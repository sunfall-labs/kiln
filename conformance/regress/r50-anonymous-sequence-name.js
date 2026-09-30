const plain = { n: (0, function () {}) };
const spread = { ...{}, n: (0, function () {}) };
const arrow = { n: (0, () => 1) };
const cls = { n: (0, class {}) };
let calls = 0;
const effect = { n: (++calls, function () {}) };
console.log(JSON.stringify([plain.n.name, spread.n.name, arrow.n.name, cls.n.name, effect.n.name]), calls);
console.log(({ n: function () {} }).n.name, ({ n: class {} }).n.name);
