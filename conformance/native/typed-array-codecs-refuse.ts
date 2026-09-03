// The base64 and hex codecs on `Uint8Array.prototype`, which refuse rather than read undefined.
//
// WHY A PROBE FOR SOMETHING THAT REFUSES. Reading an unimplemented member off a typed-array
// *instance* already refused; reading the same member off the *prototype object* answered
// `undefined`, and the prototype is where a library that patches or feature-detects a method
// looks. `Uint8Array.prototype.toBase64` and `toSorted` were both wrong that way.
//
// The kind is part of the question and this probe says so. `toBase64` and its three siblings read
// and write *bytes*, so they are `Uint8Array`'s alone: on an `Int32Array` the name is absent in
// the language and `undefined` is its right answer, and refusing there would be a stop where the
// interpreter answers. The list that decides is `kiln_c99_is_typed_array_member`.
console.log("Int32Array.prototype.toBase64", typeof Int32Array.prototype["toBase64" as never]);
console.log("Uint8Array.prototype.slice", typeof Uint8Array.prototype.slice);
console.log("Uint8Array.prototype.toBase64", typeof Uint8Array.prototype["toBase64" as never]);
console.log("unreachable");
