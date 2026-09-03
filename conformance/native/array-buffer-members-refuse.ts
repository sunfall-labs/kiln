// `ArrayBuffer.prototype.slice` reached off a buffer, which refuses rather than reading undefined.
//
// WHY A PROBE FOR SOMETHING THAT REFUSES. An `ArrayBuffer` here answers `byteLength` and
// `constructor`; every other member of it read as `undefined` against the interpreter's functions
// and booleans, and `typeof buffer.slice === "function"` is a guard a bundle really writes before
// it copies bytes. No inventory found these, because a sweep that walks `ArrayBuffer.prototype`
// never reaches an instance.
//
// The decision is to refuse: none of `slice`, `resize`, `transfer`, `detached`, `maxByteLength`
// or `resizable` is implemented, and this buffer has no length that can move. The list that
// decides is `kiln_c99_is_array_buffer_member` in the emitted C, and the day one of them is
// implemented this probe starts matching, which is the right outcome.
const buffer = new ArrayBuffer(8);
console.log("byteLength", buffer.byteLength);
console.log("constructor", buffer.constructor === ArrayBuffer);
console.log("absent name", typeof (buffer as unknown as Record<string, unknown>)["nope"]);
console.log("slice", typeof buffer.slice);
console.log("unreachable");
