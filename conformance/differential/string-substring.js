// `substring` clamps and swaps; `slice` counts from the end and yields nothing when the ends
// cross; `substr` takes a count. They look alike and agree on almost no edge case.
const s = "abcdef";
console.log(s.substring(1, 3), s.substring(3, 1), s.substring(-1, 2), s.substring(2));
console.log(s.substring(10), s.substring(0, 99), s.substring(NaN, 2), s.substring(2, undefined));
console.log(s.substr(1, 3), s.substr(-2), s.substr(-2, 1), s.substr(2), s.substr(1, -1));
console.log(s.slice(1, 3), s.slice(-2), s.slice(3, 1), s.slice(-99, 99));
console.log("".substring(0, 1), "".substr(0, 1), "".slice(0, 1));

// A regexp prints as its source and flags, which is what a library comparing them relies on.
console.log(/a/.toString(), String(/b/gi), new RegExp("c", "m").toString());
console.log(RegExp.prototype.test.call(/l+/, "hello"));
console.log(RegExp.prototype.exec.call(/l+/, "hello")[0]);
console.log(Date.prototype.toISOString.call(new Date(0)));
