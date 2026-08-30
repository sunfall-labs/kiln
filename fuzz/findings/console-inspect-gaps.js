// FOUND BY `soak --node-screen --cases 4000 --seed 101`. Three separate gaps in one area, each
// reduced to one line.
//
// **ALL THREE ARE WRONG IN BOTH ENGINES.** That is expected here and is not a coincidence: NOTES.md
// records that `kiln_c99_inspect` is a deliberate C port of the interpreter's `inspect`, so the two
// implementations agree by construction and the interpreter/native comparison can never see a gap
// in what they were both told to do. `conformance/differential/console-inspect.js` covers this area
// against node and passes; these are three shapes it does not contain.
//
// 1. A symbol-keyed property is not shown at all.
//      node        { Symbol(Symbol.toPrimitive): [Function: [Symbol.toPrimitive]] }
//      both        {}
//
// 2. An object whose single-line form exceeds node's break length is not broken across lines.
//      node        {\n  p0: 'aaaaaaaaaaaaaaaa',\n  p1: ...\n}
//      both        { p0: 'aaaaaaaaaaaaaaaa', p1: ... }   (one line)
//
// 3. Depth is counted one level too eagerly for an array reached through objects. Both agree with
//    node on `[[[[1]]]]` and on `{p0:{p1:{p2:{p3:1}}}}`, so the limit itself is right; it is where
//    an array nested under objects sits relative to it that is not.
//      node        { p0: { p1: [ [] ] } }
//      both        { p0: { p1: [ [Array] ] } }
//
// Verify with:
//   cd fuzz && cargo run --release --bin differ -- findings/console-inspect-gaps.js
console.log({ [Symbol.toPrimitive]: () => 1 });
console.log({ p0: "aaaaaaaaaaaaaaaa", p1: "bbbbbbbbbbbbbbbb", p2: "cccccccccccccccc", p3: "dddddddddddddddd" });
console.log({ p0: { p1: [ [] ] } });
