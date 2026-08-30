// FOUND BY `soak --node-screen --cases 4000 --seed 101`, and by far the most common finding in
// that run: 828 of 4,000 generated programs reached it.
//
// **BOTH ENGINES ARE WRONG IN THE SAME WAY, so the interpreter-versus-native comparison sees
// agreement and reports a pass.** This is the class `fuzz/src/triage.rs` was written to expose,
// and NOTES.md records the same shape twice before, in the project's own words: "Both engines were
// wrong in the same way, which is why no corpus caught it: they agreed."
//
// When neither `valueOf` nor `toString` returns a primitive, ToPrimitive has no answer and step 6
// of OrdinaryToPrimitive raises a TypeError. kiln invents `[object Object]` instead -- so a
// program the specification says must stop keeps running, and everything it prints afterwards is
// output node never produces at all.
//
//   node:                   TypeError: Cannot convert object to primitive value (exit 1, no stdout)
//   interpreter and native: [object Object]1
//
// Verify with:
//   cd fuzz && cargo run --release --bin differ -- \
//       findings/no-primitive-available-does-not-throw-minimal.js
console.log({ valueOf: () => ({}), toString: () => ({}) } + 1);
