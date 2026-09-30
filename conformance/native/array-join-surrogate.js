// Array.prototype.join must preserve arbitrary UTF-16 code units. lz-string's
// compression output is binary data represented as JavaScript strings, including
// lone surrogates, so routing through UTF-8 corrupts its results.
const high = String.fromCharCode(0xd800);
const low = String.fromCharCode(0xdc00);
const joined = ["a", high, low, "b"].join("");
console.log(joined.length, joined.charCodeAt(1), joined.charCodeAt(2));
