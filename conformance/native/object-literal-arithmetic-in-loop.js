// kiln-native: profile=native-release
//
// `.scratch/realms-and-concurrency` issue 16: an object literal whose property is a product of
// two integers, built in a loop, did not compile under the release profile. The product of two
// integers is not proven to stay one, so the emitter writes it into the value's double twin --
// and nothing else read that twin, so the liveness pass never declared it: the C named
// `d16` and stopped. The sum and the difference take the same path, and the values past 2^31
// and 2^53 check that the copy in the literal is the double, not a truncated integer.
function build(count, scale) {
  const out = [];
  for (let i = 0; i < count; i += 1) {
    out.push({ code: i * 3, product: i * scale, sum: i + scale, difference: i - scale, label: "item-" + i });
  }
  return out;
}

for (const scale of [3, 65536, 2147483647, 4503599627370496]) {
  const items = build(13, scale);
  const last = items[items.length - 1];
  console.log(scale, last.code, last.product, last.sum, last.difference, last.label, items[4].product);
}
