// Bounded companion for the large deep-recursion and parallel-refused probes. Their ordinary
// parity cases keep the full workload; this one makes collection on every allocation practical.
function descend(n) {
  const local = [n, { value: n }];
  try {
    return n === 0 ? 0 : local[1].value + descend(n - 1);
  } finally {
    if (local[0] !== n) throw new Error('recursion local changed');
  }
}
if (descend(48) !== 1176) throw new Error('recursive result changed');

const values = Array.from({ length: 24 }, (_, index) => index);
const seen = [];
const mapped = values.parallelMap((value) => {
  seen.push(value); // Forces the sequential fallback and records callback order.
  return { value, pair: [value, value + 1] };
});
if (seen.length !== values.length || mapped.length !== values.length)
  throw new Error('fallback length changed');
for (let index = 0; index < values.length; index++) {
  if (seen[index] !== index || mapped[index].value !== index ||
      mapped[index].pair[0] !== index || mapped[index].pair[1] !== index + 1)
    throw new Error('fallback value changed');
}
const shared = { label: 'shared', values: [3, 5, 8] };
const copied = structuredClone(Array.from({ length: 16 }, (_, index) => ({ index, shared })));
if (copied.length !== 16 || copied[15].shared !== copied[0].shared ||
    copied[0].shared === shared || copied[0].shared.values[2] !== 8)
  throw new Error('bounded clone changed sharing');
console.log('recursive and fallback roots survived');
