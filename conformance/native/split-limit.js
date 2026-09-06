// `split`'s second argument. `ToUint32` is why a limit of zero answers the empty array (even
// with no separator at all), a negative one wraps to a count no real split reaches -- so it
// reads as "unlimited" -- and NaN converts to zero, which is the one limit that answers empty
// where a naive clamp-to-zero-then-treat-as-"no limit" would have answered the whole split.
console.log("a b c".split(" ", 0));
console.log("a b c".split(undefined, 0));
console.log("a b c".split(" ", 1));
console.log("a b c".split(" ", 2));
console.log("a b c".split(" ", 3));
console.log("a b c".split(" ", 4));
console.log("a b c".split(" ", 100));
console.log("a b c".split(" ", -1));
console.log("a b c".split(" ", NaN));
console.log("a b c".split(" ", Infinity));
console.log("a  b".split(" ", 2));
