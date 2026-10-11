// kiln-native: stress=skip
// Array.prototype.sort merges runs through a scratch buffer. An element parked there is
// reachable from nowhere else, and the comparator allocates, so the collector must see it.
// Shaped after typed-function's sort(compareSignatures) over a thousand signature objects.
function make(n) {
  const items = [];
  for (let i = 0; i < n; i++) {
    items.push({ params: [(i * 7) % 13, i, 'p' + i], name: 'sig' + i });
  }
  return items;
}
for (const n of [31, 100, 700, 1215]) {
  const items = make(n);
  let missing = 0;
  let churn = 0;
  items.sort((a, b) => {
    if (!a || !b || !a.params || !b.params) { missing++; return 0; }
    const waste = [];
    for (let k = 0; k < 24; k++) waste.push({ k, s: 'w' + k + a.name });
    churn += waste.length;
    return a.params[0] - b.params[0];
  });
  const names = new Set(items.map(x => x && x.name));
  let ordered = true;
  for (let i = 1; i < n; i++) if (items[i - 1].params[0] > items[i].params[0]) ordered = false;
  console.log(n, 'missing', missing, 'distinct', names.size, 'ordered', ordered, 'churn>0', churn > 0);
}
