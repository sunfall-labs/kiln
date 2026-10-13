// Repetition copies UTF-16 code units, even when pairs span repetition boundaries.
const units = value => Array.from({length:value.length}, (_,i) => value.charCodeAt(i));
const samples = ['', 'a\u0000\u00ff', '\u0100\ufffd', '\ud800', '\udfff',
  'a\ud83dx\ude00', '\ud83d\ude00', '\udc00\ud800', '\ud800\ud800\udc00'];
for (let i=0; i<samples.length; i++) {
  for (const count of [0, 1, 2, 3]) {
    const value = samples[i].repeat(count);
    console.log(i, count, JSON.stringify(units(value)));
  }
}
// A managed receiver and allocations after the result exercise collection at both ends.
const source = ['a', '\ud83d', 'x', '\ude00'].join('');
const large = source.repeat(1024);
const scratch = Array.from({length:32}, (_,i) => ({value:source.repeat(i)}));
let checksum = 0;
for (let i=0; i<large.length; i++) {
  if (large.charCodeAt(i) !== source.charCodeAt(i % source.length)) {
    throw new Error('repeat changed unit '+i);
  }
  checksum = (checksum + large.charCodeAt(i)) >>> 0;
}
console.log('large', large.length, checksum, scratch[31].value.length);
