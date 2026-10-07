# Native String wrapper own properties

The native `string-empty-construction.js` probe crashed, and `primitive-wrappers.js` reported a
zero length for `new String("A😀")`. Both failures came from constructing String wrapper own
properties through ordinary `[[Set]]`. `String.prototype` has an inherited, non-writable `length`
property, so the sloppy write returned success without creating the wrapper's own `length`; the
empty case then indexed `properties[count - 1]` with `count == 0`.

`kiln_c99_new_primitive_wrapper` now creates the indexed characters and `length` through
`kiln_c99_own_slot` and stores their prescribed descriptor bits directly. Indices remain
enumerable, non-writable and non-configurable; `length` remains non-enumerable, non-writable and
non-configurable. This bypasses inherited setters and read-only properties as own-property
creation requires.

The production change is commit `cbcbaa409460d43615b3602a593b2e3a6d2c011b`, based on source
`fc5d866d82ca4965176ad75634396e5edeb27c4a`. The candidate CLI is SHA-256
`d3914602ebf2d7afd12d918c8adf0573be20b775df5a2f22bb32ca57397f4713`; it was built with pinned
Rust 1.97.1 using `mbx build -p kiln-cli --release --offline`. Its native provider manifest is
SHA-256 `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b` (ABI 82).

The original probes now both exit 0 and match Node byte-for-byte:

- `string-empty-construction.js`: `true true 0 x  0`
- `primitive-wrappers.js`: full output is retained in `logs/fixed-wrappers.stdout` and matches
  `logs/node-wrappers.stdout`, including `0,1,2 3` for the UTF-16 code units of `A😀`.

Both same binaries also pass with `KILN_NATIVE_GC_STRESS=1`. The focused C fixture verifies that
the wrapper definition helper creates an own, non-enumerable, non-writable and non-configurable
`length` value over an inherited non-writable `length`. `mbx test -p kiln-backend-c --lib
string_wrapper_definition_shadows_inherited_readonly_length --offline -- --test-threads=1` passed
1/1, and `cargo fmt --all -- --check` passed.

The baseline empty-wrapper binary (CLI58, SHA-256
`96c5ae2bc1d6f988b7fdc52a583044e77c32eec6d9529ce3c49f499dc7f71ef1`) crashed at
`kiln_c99_new_primitive_wrapper+1715`. GDB showed the post-`kiln_c99_put("length")` object still
had its initialized wrapper brand but `properties == NULL` and `count == 0`; no collection had
cleared the object. This rules out the initial GC-lifetime hypothesis and confirms the ignored
inherited-read-only write. The raw build, GDB, focused test and probe outputs are retained in
`logs/`.
