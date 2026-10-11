// The bounded companion of sort-scratch-rooted.js, small enough to run under
// KILN_GC_STRESS: sorts of 32 or more elements merge runs through a scratch buffer, and with a
// collection on every allocation an element parked only in that buffer is reclaimed within a
// few comparisons.
for (const n of [40, 100]) {
  const a = [];
  for (let i = 0; i < n; i++) a.push({ params: [i % 7, i], name: 'n' + i });
  let bad = 0;
  a.sort((x, y) => {
    const t = [x && x.params, y && y.params, { a: 1 }, 'x' + bad];
    if (!x || !y || !x.params || !y.params || t.length !== 4) { bad++; return 0; }
    return x.params[0] - y.params[0];
  });
  console.log(n, bad, a.length, new Set(a.map(x => x.name)).size, a.map(x => x.params[0]).join('').slice(0, 20));
}
