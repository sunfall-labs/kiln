// `indexOf`'s `fromIndex` and the places `substring` disagrees with a naive `slice`: swapped
// bounds get swapped back, a negative or NaN bound clamps to zero rather than counting from the
// end, and an out-of-range bound clamps to the length. The non-ASCII receiver checks that a
// found position is counted in UTF-16 units, not in the bytes the fast path uses when it can.
const s = "abcabcabc";
console.log(s.indexOf("a"), s.indexOf("a", 1), s.indexOf("a", 3), s.indexOf("a", -1));
console.log(s.indexOf("a", 100), s.indexOf("a", NaN), s.indexOf(""), s.indexOf("", 100));
console.log(s.indexOf("", -1), s.indexOf("z"));

console.log(s.substring(3, 1), s.substring(1, 3), s.substring(-3, 2), s.substring(2, -3));
console.log(s.substring(NaN, 3), s.substring(3, NaN), s.substring(2, 100), s.substring(100, 2));
console.log(s.substring(2), s.substring(-1));

const accented = "café123";
console.log(accented.indexOf("1"), accented.indexOf("é"), accented.substring(accented.indexOf("1")));
console.log(accented.length, accented.substring(0, accented.length) === accented);
