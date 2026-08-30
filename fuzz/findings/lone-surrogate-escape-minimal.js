// FOUND BY `soak --node-screen --cases 4000 --seed 101`. The string `"\ud800"` is already in
// `js.rs`'s `STRINGS` pool, chosen for ADR 0022's reasons, and it took a third reference to notice
// that kiln does not read it.
//
// **BOTH ENGINES ARE WRONG IN THE SAME WAY**, and necessarily so: this is the frontend's reading
// of a string literal, which both engines inherit. There is no arrangement of kiln components that
// could have caught it -- the interpreter and the native backend agree because they are given the
// same wrong string.
//
// `"\ud800"` is a valid string literal containing one code unit, the lone high surrogate U+D800.
// kiln reads it as five: a replacement character followed by the literal text `d800`.
//
//                        node   interpreter and native
//   "\ud800".length      1      5
//   "a\ud800b".length    3      7
//
// Verify with:
//   cd fuzz && cargo run --release --bin differ -- findings/lone-surrogate-escape-minimal.js
console.log("\ud800".length);
