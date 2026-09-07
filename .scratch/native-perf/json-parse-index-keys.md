# A parsed integer-like key keeps source order natively

**Role:** implementer. **State:** open. **Found:** 2026-09-07, while making `JSON.parse` faster.

    JSON.stringify(JSON.parse('{"":1,"0":2,"01":3}'))

    node        {"0":2,"":1,"01":3}
    interpreter {"0":2,"":1,"01":3}
    native      {"":1,"0":2,"01":3}

It is not one example, and it is not only `JSON.stringify` -- `Object.keys` of the parsed object
answers the same wrong order:

| input | node and the interpreter | native |
| --- | --- | --- |
| `{"":1,"0":2,"01":3}` | `{"0":2,"":1,"01":3}` | insertion order |
| `{"b":1,"2":2,"a":3,"1":4,"0":5}` | `{"0":5,"1":4,"2":2,"b":1,"a":3}` | insertion order |
| `{"4294967295":1,"4294967296":2,"10":3}` | `{"10":3,"4294967295":1,"4294967296":2}` | insertion order |
| `{"-1":1,"1":2}` | `{"1":2,"-1":1}` | insertion order |

The last two rows are the boundary cases worth keeping: `4294967295` is an index and
`4294967296` is not, and `-1` is not.

An own property whose key is an array index is enumerated before every string key, in ascending
numeric order (ECMA-262 §7.3.24 OrdinaryOwnPropertyKeys). `"0"` is an index; `""` and `"01"` are
not. The native backend's `JSON.parse` leaves an index-like key in the object's shape rather than
its element store, so the object enumerates in the order the document listed.

Pre-existing: it reproduces on `875c4ccc` and earlier, and neither the shaped-build work nor the
formatter touched it. Documented in a comment in `conformance/native/json-shaped-build-edges.js`
rather than tripped, so the corpus does not carry a known-diverging file.

**Fix:** route an index-like key into `elements` while parsing. That costs a test per key on the
parse path, which is why it belongs in its own change with its own measurement rather than being
folded into a performance one.
