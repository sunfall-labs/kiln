// Integer-specialized callbacks must retain NaN and negative zero when their
// results are boxed into the output array. Inspect the stored values afterward.
function dynamicRemainder(value, index) {
  return (value | 0) % (index | 0);
}
function constantRemainder(value) {
  return (value | 0) % 2;
}
function describe(value) {
  if (Number.isNaN(value)) return "NaN";
  if (Object.is(value, -0)) return "-0";
  return String(value);
}
const input = [-4, -0, -4, NaN, Infinity, 4, -6];
console.log(input.map(dynamicRemainder).map(describe).join(","));
console.log(input.map(constantRemainder).map(describe).join(","));
