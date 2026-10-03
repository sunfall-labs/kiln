// UTF-16 read counts, indivisible UTF-8 writes, view bounds and Web IDL brands.
const encoder = new TextEncoder();
for (const text of ['abc', 'é', '€', '😀', '\ud800x', '\udc00', 'a😀é']) {
  for (let capacity = 0; capacity <= 8; capacity++) {
    const backing = new Uint8Array(capacity + 2).fill(99);
    const result = encoder.encodeInto(text, backing.subarray(1, capacity + 1));
    console.log(JSON.stringify([text, capacity, result, Array.from(backing)]));
  }
}
console.log(encoder.encoding, Object.prototype.toString.call(encoder));
console.log(Object.keys(TextEncoder.prototype).join(','), TextEncoder.name,
  TextEncoder.prototype.encode.length, TextEncoder.prototype.encodeInto.length);
for (const invoke of [
  () => encoder.encodeInto('x'),
  () => encoder.encodeInto(Symbol(), new Uint8Array(4)),
  () => encoder.encodeInto('x', new Uint8ClampedArray(4)),
  () => TextEncoder.prototype.encodeInto.call({}, 'x', new Uint8Array(4)),
  () => TextEncoder.prototype.encode.call({}, 'x'),
]) {
  try { invoke(); console.log('unexpected'); } catch (error) { console.log(error.name); }
}
console.log(Array.from(encoder.encode('é😀\ud800')).join(','));
