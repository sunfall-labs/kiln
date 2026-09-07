// Every double has exactly one JavaScript spelling, and this probe asks for 200,000 of them.
//
// `Number::toString` (ECMA-262 6.1.6.1.20) is not "print seventeen digits and trim": the answer
// is the *shortest* digit string that reads back as this exact double, nearest among the
// candidates of that length, ties to the even digit -- and then laid out by rules that switch to
// exponential notation at twenty-one digits above the point and six zeros below it. A formatter
// can be wrong about any of those three things for a thin sliver of the 2^64 bit patterns and
// look completely right on the numbers anyone types by hand.
//
// So the inputs here are bit patterns, not numbers: a deterministic xorshift fills a DataView and
// the double comes back out of it, which reaches the subnormals, the powers of two, the
// boundaries between the layout rules and the values whose shortest form is seventeen digits.
// NaN payloads are skipped -- all 2^53 of them spell "NaN" and they would crowd out the patterns
// that decide anything.
//
// The output is one FNV-1a hash of every spelling joined by commas, so `node` and the native
// binary can be compared in one line, plus the first two hundred spellings verbatim so that a
// mismatch names a value instead of only a number. The hash is 64-bit and printed as two 32-bit
// halves: a JavaScript number carries 53 bits exactly and a 64-bit hash printed as one number
// would agree in the bits nobody keeps.

// FNV-1a, 64-bit, over the bytes of the spellings. The multiply by 0x100000001b3 is done in two
// halves because every partial product then stays under 2^42 and is exact in a double: the low
// half is `lo * 0x1b3` truncated, and the high half collects `hi * 0x1b3`, `lo * 0x100` and the
// carry the low half dropped.
let hashHigh = 0xcbf29ce4;
let hashLow = 0x84222325;

function feedByte(byte) {
  hashLow = (hashLow ^ byte) >>> 0;
  const low = hashLow * 435;
  const high = hashHigh * 435 + hashLow * 256 + Math.floor(low / 4294967296);
  hashLow = low >>> 0;
  hashHigh = high >>> 0;
}

let seen = 0;
const firstSpellings = [];

function record(text) {
  if (seen !== 0) {
    feedByte(44); // the comma between spellings
  }
  for (let i = 0; i < text.length; i++) {
    feedByte(text.charCodeAt(i));
  }
  if (seen < 200) {
    firstSpellings.push(text);
  }
  seen++;
}

function spell(value) {
  record(String(value));
}

// -- the values a person would think to try ------------------------------------------------

const edges = [
  0, -0, 1, -1, 0.1, 0.2, 0.3, 1 / 3, 2 / 3,
  1e21, 1e21 - 65536, 1e20, 123456789012345680000,
  1e-6, 1e-7, 0.000001, 0.0000001,
  5e-324, 2.2250738585072014e-308, 1.7976931348623157e308,
  9007199254740991, 9007199254740992, 9007199254740993,
  4294967295, 4294967296, 9007199254740992 + 2,
  1.5, 2.5, 0.5, 100, 1e100,
  1.7976931348623157e+308, 4.9406564584124654e-324,
  1.2345678901234567e-7, 123456.789, 0.1 + 0.2
];
for (let i = 0; i < edges.length; i++) {
  spell(edges[i]);
}

// The powers of two, built by halving and doubling rather than by `Math.pow`, so the value is
// exact by construction and not by trusting a library function that both engines would have to
// agree about before this probe could say anything about spelling.
let up = 1;
for (let power = 0; power <= 330; power++) {
  spell(up);
  up = up * 2;
}
let down = 1;
for (let power = -1; power >= -330; power--) {
  down = down / 2;
  spell(down);
}

// The powers of ten, through the string form, because repeated multiplication by ten is not
// exact and `1e-330` has to be the correctly rounded subnormal rather than whatever a chain of
// roundings arrives at. `1e330` overflows to Infinity and is dropped: "where finite".
for (let power = -330; power <= 330; power++) {
  const value = +("1e" + power);
  if (value !== value || value === Infinity || value === -Infinity) {
    continue;
  }
  spell(value);
}

// -- 200,000 bit patterns ------------------------------------------------------------------

// xorshift32 (Marsaglia), seeded by hand: the point is that node and the native binary walk the
// identical sequence, so nothing here may reach `Math.random`.
let state = 0x9e3779b9;

function next32() {
  let x = state;
  x = (x ^ (x << 13)) >>> 0;
  x = (x ^ (x >>> 17)) >>> 0;
  x = (x ^ (x << 5)) >>> 0;
  state = x;
  return x;
}

const view = new DataView(new ArrayBuffer(8));
let drawn = 0;
while (drawn < 200000) {
  view.setUint32(0, next32());
  view.setUint32(4, next32());
  const value = view.getFloat64(0);
  if (value !== value) {
    continue; // a NaN payload; every one of them spells "NaN".
  }
  spell(value);
  drawn++;
}

console.log("spellings " + seen);
console.log("fnv1a64 " + hashHigh + " " + hashLow);
for (let i = 0; i < firstSpellings.length; i++) {
  console.log(i + " " + firstSpellings[i]);
}
