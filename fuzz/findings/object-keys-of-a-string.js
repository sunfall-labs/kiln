// FOUND BY `soak --node-screen --cases 4000 --seed 101`. Reduced from three separate generated
// programs that reached it through `Object.keys`, `Math.abs(Object.keys(...))` and an array
// literal.
//
// **`Object.keys` of a string returns `[]`.** A String object's own enumerable properties are its
// index keys, so `Object.keys("abc")` is `[ '0', '1', '2' ]`. kiln's interpreter answers `[]`, and
// then everything downstream inherits the wrong length: `Math.abs(Object.keys("t0"))` is `NaN` in
// node -- a two-element array does not convert to a number -- and `0` in kiln, because an empty
// array does.
//
// Arrays are handled correctly, which is what makes this specific rather than general.
//
//                              node                interpreter     native
//   Object.keys("abc")         [ '0', '1', '2' ]   []              refused (-2)
//   Object.keys("")            []                  []              refused (-2)
//   Object.keys([7, 8])        [ '0', '1' ]        [ '0', '1' ]    refused (-2)
//   Math.abs(Object.keys("t0"))  NaN               0               refused (-2)
//
// The native column is the growing native surface and not a second answer: the emitted runtime
// returns KILN_C99_CONTROL_ERROR for this program, which is a backlog entry that names itself.
//
// Verify with:
//   cd fuzz && cargo run --release --bin differ -- findings/object-keys-of-a-string.js
console.log(Object.keys("abc"));
console.log(Object.keys(""));
console.log(Object.keys([7, 8]));
console.log(Math.abs(Object.keys("t0")));
