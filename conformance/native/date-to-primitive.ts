// A date is the one built-in whose `ToPrimitive` answer depends on which hint was asked for:
// "default" reads as a string and "number" as an instant. That is the whole of why `date + date`
// concatenates two date strings while `date - date` subtracts two instants, and reducing a date
// with a two-valued flag got one of those two wrong in both engines at once.
const d = new Date(86400000);
const e = new Date(86400000);

console.log(typeof (d + d));
console.log((d + d) === (String(d) + String(d)));
console.log(d - d);
console.log(d * 2);
console.log(-d, +d);

// The relational operators ask for a number, so these compare instants rather than date strings.
console.log(d < e, d <= e, d > e, d >= e);

// `==` reduces only across a type boundary: two dates compare as themselves, a date against a
// string reduces to that string, and a date against its own instant does not -- the default hint
// hands `==` a string, and that string is not a number.
console.log(d == e, d === e);
console.log(d == String(d));
console.log(d == 86400000);

// `${}` and `String()` ask for a string outright, which is the same answer by a different route.
console.log(`${d}` === String(d), ("" + d) === String(d));
console.log(Number(d), Boolean(d));
console.log([d].join("|") === String(d));
