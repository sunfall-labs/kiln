# 23. An array's string conversion does not call its elements' `toString`

Status: resolved

Found while writing issue 19's probe. `Array.prototype.toString` is `join`, and `join` applies
`ToString` to each element, which for an object calls its `toString` (or `Symbol.toPrimitive`).
Both engines skip that when the array itself is converted, and the native backend skips it in
`join` too. Every profile; unrelated to the release source DCE.

```js
const seen = [];
const o = { toString() { seen.push("t"); return "y"; } };
console.log([o] + "", `${[o]}`, String([o]), [o].join(), seen.join());
// node:             y y y y t,t,t,t
// interpreter:      [object Object] [object Object] [object Object] y t
// native (dev):     [object Object] [object Object] [object Object] [object Object]
```

## Acceptance

- The line above prints node's answer in the interpreter and at `native-dev` and `native-release`;
  a probe under `conformance/native` holds it (`[o] + ""`, a template, `String`, `join`, an element
  whose `toString` throws, `Symbol.toPrimitive` on an element).

## Comments

2026-09-27, branch `work/parity-23-25`.

**Root cause**, one per engine. The interpreter's `join` was right; `Array.prototype.toString`
was not. `dispatch_native` sent it to the generic walk (which calls the receiver's own `join`)
only for a typed array, and a dense array fell to the pure arm in `intrinsics/dispatch.rs`, which
joined with `join_units` -- a `ToString` that cannot call. So every conversion that goes through
`toString` (`[o] + ""`, a template, `String`) answered "[object Object]" while `[o].join()` was
right. Natively `KILN_C99_METHOD_JOIN` converted every non-string element with `kiln_c99_text`,
calling nothing (only a Buffer element went through `ToPrimitive`), and `ARRAY_TO_STRING` is that
`join`.

**Fix** (`interp:`/`backend-c:` commit). Interpreter: `ArrayJoinDefault` always takes the generic
arm (`vm/intrinsics.rs`), and its pure arm is `unreachable!`, as `toLocaleString`'s already is.
Native (`runtime/dispatch_part_b.c`, an existing C path, no new built-in): each non-string element
goes through `kiln_c99_to_text_argument` -- `ToPrimitive` with the string hint, a symbol a
`TypeError` -- and the walk captures the length once and reads a vanished index as `undefined`,
since an element's `toString` can shorten the array.

**Probes.** `conformance/regress/r39-array-element-tostring.js` (expected stdout from node
26.8.1): the repro; `@@toPrimitive` on an element (with its `"string"` hint); a `valueOf`-only
element; nested arrays; a throwing `toString` through `join` and through a template; a symbol
element; an element that shortens the array mid-join. The same program is
`conformance/native/array-element-tostring.js` at `native-release`. By hand, the repro and the
probe match node in the interpreter and at `native-dev` and `native-release`; the base binary
diverged in all three.

**Results** (branch's release CLI, worktree `kiln-wt-parity3`). `mbx test -p kiln-ir -p
kiln-interpreter -p kiln-backend-c`: 69 + 200 + 203 + 36 passed, 0 failed (the one change to a
snapshot is issue 25's ABI surface). `scripts/native.sh`: 286 matched, 1 refused (`shadow-realm`,
as before), 0 diverged. `kiln test conformance/regress --node`: 41/41 pass.
