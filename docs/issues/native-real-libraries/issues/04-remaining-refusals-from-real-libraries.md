# The rest of the native refusals, by library

Status: ready

Recorded verbatim from the first native run of the corpus so the one-table work can be checked
against real demand. Several will fall out of it; the ones that will not are noted.

| Library | Refusal |
| --- | --- |
| `ajv` | `New` -- target unsupported by the native constructor model |
| `zod` | `New` -- target unsupported by the native constructor model |
| `dayjs` | `GetComputed` |
| `immer` | `GetProperty(from)` on a builtin |
| `lodash` | `GetProperty(getOwnPropertySymbols)` -- see issue 02 |
| `marked` | `CallMethod(iterator)` -- diagnosed and fixed, see below |
| `mobx` | `Call` -- callee unsupported by the native callable model |
| `rxjs` | `GetProperty(observable)`, since moved into the turn loop -- see below |
| `vue-reactivity` | `CallMethod(getOwnPropertyNames)` -- see issue 02 |
| `async-concurrency` | unsupported native emission: the `Performance` builtin |

`async-concurrency` also diverges *interpreted*, on the `[Function: returnSignal]` sentinel; that
is a separate, older problem and not native's.

`Performance` is a decision rather than a gap: decide whether the native backend gets a clock
surface at all before implementing one, since ADR 0015 makes the realm's clock virtual.

## `marked`: closed

`CallMethod(iterator)` was `CallMethod(@@iterator)` with the `@` filtered out of the diagnostic,
and `receiver_tag=11` was the *method's* tag rather than the receiver's -- the
`CallMethod invocation refused` branch records `method.tag`, so 11 meant the lookup succeeded and
the call is what stopped. The shape is `for (const s of someString)`:
`KILN_C99_METHOD_ARRAY_VALUES` answered a string's `@@iterator` and then refused a string
receiver, while spread walked one correctly through `kiln_c99_iterate`. Fixed, with a
`conformance/native` probe that asks both spellings of the walk about the same strings.

## `rxjs`: still open, but no longer anonymous

The refusal moved: it is now inside `kiln_realm_drive`, not the module body, and reported as
`native backend refusal in the turn loop`. That phrase is a *phase*, not a name, and the reason is
structural rather than mysterious.

`kiln_c99_invoke_closure_body` composes a full diagnostic into the call result of any generated
body that refuses, and `kiln_c99_note_refusal` records site, instruction and operation in the
realm as it goes. None of that reached the report: the three work callbacks the drive runs --
`kiln_c99_run_microtask`, `kiln_c99_run_next_tick`, `kiln_c99_run_timer` -- returned the status and
dropped the value, and the thrown slot the drive was handed is the only channel they share with
the embedding. They now publish the refusal into it, so the next run of `scripts/thirdparty.sh`
should print `native backend refusal: module=… instruction=… operation=…` for `rxjs` instead of
the phase. **This has not been run**; it is a static fix and wants confirming.

Three guards inside the turn machinery refused with nothing to say at all, because they run a turn
after the code that scheduled them and so have no instruction to be attributed to. Each now sets
`kiln_c99_diagnostic_detail`: `@@adopt` handed something other than a pair of promises, a promise
combinator's per-element slot that is not the object it builds, and `@@resume`/`@@throw` resuming
against something that is not a suspended frame.

Two things found while reading that are *not* fixed, both inferred from source and unmeasured:

- `kiln_c99_settle` adopts only a `KILN_VALUE_PROMISE`. A thenable that is not a promise is
  fulfilled *with the thenable object* rather than followed, so `await thenable` and
  `Promise.resolve(thenable)` would answer the object. That is a silent wrong answer rather than a
  refusal, which makes it worse than what `rxjs` currently hits; it is worth a probe of its own.
- `KILN_C99_METHOD_ITERATOR_NEXT` refuses when `@@iterated` is neither an array nor a typed array,
  which is how an async-iteration step over anything else would end -- a candidate for
  `from-async-iter`, and cheap to test with a one-line probe once someone can run things.

## `lodash`, `immer` and `mobx`: the shared `CallMethod(call)` refusal, closed

All three stopped on `operation=CallMethod(call) detail=CallMethod invocation refused
receiver_tag=11`, at instructions 11, 37 and 4 of their module bodies. Bisected through the
emitted C (`KILN_NATIVE_KEEP_WORK=1` keeps `module-0.c`, and the refusal's site and instruction
index locate the `kiln_c99_note_refusal` call exactly) rather than from the diagnostic text, which
names only the borrow and not the borrowed method.

One cause, three receivers. Every one of them was
`Object.prototype.hasOwnProperty.call(receiver, key)`:

| Library | Function | Receiver |
| --- | --- | --- |
| `lodash` | `getRawTag` | `new Map()` -- a `KILN_VALUE_COLLECTION` |
| `immer` | `isPlainObject` | `Array.prototype` -- a `KILN_VALUE_BUILTIN` |
| `mobx` | `hasProp` | whatever `hasProp` was handed |

`KILN_C99_METHOD_HAS_OWN_PROPERTY` answered for an object, an array, a typed array and a
typed-array prototype, and refused every other receiver. `Object.hasOwn` -- the same question, a
different spelling -- had a *different* list: it also answered for a function and for a proxy.
Neither converted a symbol key to the slot a symbol keys under, so
`has.call({[Symbol.toStringTag]: "W"}, Symbol.toStringTag)` answered false where both node and the
interpreter say true. The two are now one helper, `kiln_c99_own_property_present`.

Minimal reproducer, measured: `Object.prototype.hasOwnProperty.call(new Map(), "size")`
refused natively and answered `false` interpreted. `conformance/native/has-own-property-borrowed.js`
covers it and every other receiver kind, and its bytes are identical under node, the interpreter
and the backend.

A string, a regular expression, an `ArrayBuffer` and a `DataView` still refuse, deliberately: the
two engines disagree about what those own (`"abc"`'s `length` and indices are own to node and to
the specification and not to the interpreter; a regexp's `source` and a buffer's `byteLength` are
own to the interpreter and not to node), and picking either answer would be a wrong one.

### Two more silent wrong answers found on the way, both fixed

- **`Function.prototype.apply` took an array, not an array-like.** `f.apply(this, arguments)` --
  how every wrapper forwards a call -- ran `f` with **no arguments at all**. `immer` builds its
  array proxy traps exactly that way, so every trap received `undefined` for its state; once the
  `hasOwnProperty` fix let `immer` reach them it died with an uncaught `TypeError` instead of a
  refusal, which for `scripts/thirdparty.sh` is a *worse* outcome than the refusal it replaced.
  `apply` now walks `length` and the indices for anything that is not an array.
- **A handled throw kept naming itself, through the realm.** `kiln_c99_forget_handled_throw`
  cleared the C statics; `kiln_c99_control_diagnostic` renders the *realm's* copy whenever a realm
  is active, and nothing cleared that. Measured: a refusal reported `detail=CallMethod invocation
  threw` about a `TypeError` two calls earlier. It now calls `kiln_realm_diagnostic_reset`.

### Where the three stop now, with a one-line reproducer each

- `lodash`: `SetComputedStrict`, still in `getRawTag`. `const m = new Map(); m[Symbol.toStringTag]
  = undefined;` refuses natively; the interpreter takes the write and then reads `"Map"` back,
  which is not node's `undefined` either.
- `immer`: `CallMethod(getOwnPropertyDescriptor)`.
  `Object.getOwnPropertyDescriptor(function f() {}, "prototype")` refuses -- the refusal
  `kiln_c99_describe_property` documents, because the backend does not record whether a given
  function's `prototype` is writable.
- `mobx`: `Binary`, which names neither the operator nor the operands.
  `"" + new Proxy({}, {})` refuses natively and is `[object Object]` interpreted. `mobx`'s
  `toPrimitive` is `typeof value === "object" ? "" + value : value` and everything it holds is a
  proxy.

### Diagnostics that misled, and are still misleading

`CallMethod(call)` was not stale here -- it was the right instruction -- but it named the *borrow*
rather than the borrowed method, and `receiver_tag=11` was `Function.prototype.call`'s own tag,
which says nothing. Two cheap improvements, neither done:

- a `Binary` refusal should set a detail with the operator and the two operand tags, the way
  `GetComputed` was taught to name its member;
- a helper that refuses with no detail lets the *caller's* instruction invent one. That is how
  `Object.getOwnPropertyDescriptor(f, "prototype")` reports `operation=Call detail=Call callee is
  unsupported by the native callable model`, which is a false statement about a callee that was
  perfectly well supported.

### Interpreter bugs found while fixing the oracle, none of them native's

Measured against node with `Object.prototype.hasOwnProperty.call(x, k)`:

- `"abc"`'s `length` and its indices are not own to the interpreter; node and the specification
  say they are.
- a regexp's `source`, an `ArrayBuffer`'s `byteLength` and a `DataView`'s `byteLength` *are* own
  to the interpreter; node puts all three on the prototype.
- `has.call({undefined: 1})` with no argument asks after `""` rather than `"undefined"`, so it
  answers false where node answers true. `Object.hasOwn({undefined: 1})` does the same. The
  backend keeps refusing that one spelling rather than pick a side.
- `has.call(Promise.resolve(1), "then")` is true interpreted and false under `Object.hasOwn` and
  under node: for the kinds it does not list, the interpreter falls through to a *chain* walk.
