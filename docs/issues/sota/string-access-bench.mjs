import { bench } from "../../bench/harness.mjs";

const shortAscii = "KilnStringAccess-0123456789";
const longAscii = "a".repeat(65536);
const unicode = ("A😀é中B").repeat(8192);

function indices(length, count, stride) {
  const result = new Array(count);
  let state = 0x9e3779b9;
  for (let i = 0; i < count; i += 1) {
    state = (state ^ (state << 13)) >>> 0;
    state = (state ^ (state >>> 17)) >>> 0;
    state = (state ^ (state << 5)) >>> 0;
    result[i] = stride === 0 ? i % length : state % length;
  }
  return result;
}

const shortSequential = indices(shortAscii.length, 4096, 0);
const shortRandom = indices(shortAscii.length, 4096, 1);
const longSequential = indices(longAscii.length, 4096, 0);
const longRandom = indices(longAscii.length, 4096, 1);
const unicodeSequential = indices(unicode.length, 4096, 0);
const unicodeRandom = indices(unicode.length, 4096, 1);

function codeUnits(text, positions) {
  let sum = 0;
  for (let i = 0; i < positions.length; i += 1) sum += text.charCodeAt(positions[i]);
  return sum;
}
function chars(text, positions) {
  let sum = 0;
  for (let i = 0; i < positions.length; i += 1) sum += text.charAt(positions[i]).length;
  return sum;
}
function atChars(text, positions) {
  let sum = 0;
  for (let i = 0; i < positions.length; i += 1) sum += text.at(positions[i]).length;
  return sum;
}

bench("short-ascii-charCodeAt-sequential", () => codeUnits(shortAscii, shortSequential));
bench("short-ascii-charCodeAt-random", () => codeUnits(shortAscii, shortRandom));
bench("long-ascii-charCodeAt-sequential", () => codeUnits(longAscii, longSequential));
bench("long-ascii-charCodeAt-random", () => codeUnits(longAscii, longRandom));
bench("unicode-charCodeAt-sequential", () => codeUnits(unicode, unicodeSequential));
bench("unicode-charCodeAt-random", () => codeUnits(unicode, unicodeRandom));
bench("short-ascii-charAt-sequential", () => chars(shortAscii, shortSequential));
bench("long-ascii-charAt-random", () => chars(longAscii, longRandom));
bench("unicode-charAt-random", () => chars(unicode, unicodeRandom));
bench("short-ascii-at-sequential", () => atChars(shortAscii, shortSequential));
bench("long-ascii-at-random", () => atChars(longAscii, longRandom));
bench("unicode-at-random", () => atChars(unicode, unicodeRandom));
