// `s[i]` is the character at that index, which is not the same as `charAt` on a bad index.
const s = "abc";
console.log(s[0], s[2], s[5], s["1"]);
console.log("xyz"[1], ""[0] === undefined);
let i = 0;
console.log(s[i], s[i + 2], s[-1], s[1.5]);

// Indices are UTF-16 code units, the same unit `length` counts in.
const pair = "a\u{1F600}b";
console.log(pair.length, pair[0], pair[3]);
console.log(pair[1] === pair.charAt(1));

// A named property of a string is not an index.
console.log(s["length"], s["nope"]);

// Iterating by index reproduces the string.
let copy = "";
for (let n = 0; n < s.length; n += 1) copy += s[n];
console.log(copy === s);
