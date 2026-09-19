# Silent `undefined` grew from 1 row to 111 between `616f9b6f` and `740f4fb4`

Status: fixed (2026-09-19, see Outcome)
Feature: native-real-libraries
Governing ADR: ADR 0005, ADR 0043
Blocks: nothing named; this is the class `AGENTS.md` calls the expensive one

## What the inventory says

`scripts/native-surface-inventory.py` re-run on 2026-09-19 at `740f4fb4` (worktree, clean
`crates/`), 601 native compiles. Against the previous run at `616f9b6f` (2026-09-03):

| Class | `616f9b6f` | `740f4fb4` |
| --- | ---: | ---: |
| diverged | 3 | **117** |
| of which read `undefined` natively | 1 | **111** (110 unexplained) |
| refused by name | 397 | 299 |
| implemented | 866 | 1304 |
| invented | 52 | 34 |
| rows | 1266 | 1720 |

Refusals fell and implemented rose -- Phase 5.2 did what it said. But the interpreter's surface
grew by a third and the native backend followed it into the wrong class: 31 rows moved from
*refused* to *silent undefined* (an honest stop became a wrong answer) and 85 rows are new
interpreter surface that native answers `undefined` for. `--gate` would fail on 110 rows today;
it is not in the gate.

The report is `../native-surface-inventory.md`, regenerated. The three tables below come from it.

## It is not read-only

Native lowers a call by name at the site, so the memo that "members break on read, not call"
suggested these might only matter to `typeof` checks. They do not. Compiled with
`--profile native-release`, with and without `--node`:

```js
console.debug("x");                       // uncaught TypeError: a undefined is not a function
                                          //   operation=CallMethod(debug) detail=non-callable callee kind
class MyErr extends Error {}
new MyErr("boom").name;                   // undefined   (node: "Error")
Error.prototype.message;                  // undefined   (node: "")
typeof Symbol.dispose;                    // "undefined" (node: "symbol")
Map[Symbol.species];                      // undefined   (node: function)
RegExp.prototype[Symbol.replace];         // undefined   (node: function)
String.prototype.length;                  // undefined   (node: 0)
globalThis.crypto.randomUUID;             // undefined   (node: function)
```

`console.time`, `table`, `assert`, `group`, `count`, `dir`, `trace` and the rest of the 18 die
the same way as `debug`. `String(new MyErr("boom"))` is right (`Error: boom`) because
`toString` has its own path; only the prototype's data properties are missing.

## Three causes, one decision each

### 1. Global `console` and `crypto` are hand-built C objects; the interpreter's are the TS built-ins (52 rows)

`globals.c:70-94` builds `console` as an ordinary object with `log`, `info`, `error`, `warn`.
`globals.c:471` builds `crypto` as an **empty** ordinary object. The interpreter serves both from
`kiln-builtins/src/node/console.ts` and `node/crypto.ts` (Phase 5.2, 2026-09-13), so its
`console` has 24 methods and a prototype (`console^`, 23 more rows) and its `crypto` has 11.

Every read past the four methods falls to `read.c:236`, "a name an ordinary object does not
own" -- a predicate that is *correct* for ordinary objects, which is why the `KILN_C99_ABSENT`
gate cannot see this: the object is the wrong kind of thing, not the predicate.

The TS built-ins already work natively: `require("node:console").debug` and
`require("node:crypto").randomUUID` both answer functions in a `--node` build, and loading
`node:crypto` even populates the global `crypto` as a side effect (`randomUUID` appears). Loading
`node:console` does not touch the global `console`. So the native global surface currently
depends on which `node:` modules something in the program happened to import.

Decision: the native global `console` and `crypto` are the same TS modules the interpreter
installs, always linked, and the two C objects in `globals.c` are deleted. This is the Phase 5.1
freeze applied to two objects it missed.

### 2. `Error.prototype` answers absent for every name, including the four it owns (21 rows)

`read.c:1550`:

```c
if (target.bits == KILN_C99_BUILTIN_ERROR_PROTOTYPE) {
    KILN_C99_ABSENT("not a member of Error.prototype; Error.prototype only owns name, message, toString, and constructor", out);
}
```

The reason names a predicate and the code checks none: `name` and `message` reach it and read
`undefined`. Arrived in `76a31245` (2026-09-03, the thirdparty e2e commit), which is why the
09-03 inventory (`616f9b6f`) still saw these as refusals. `AGENTS.md` says the reason must name
the membership predicate that *proves* the name absent; this one asserts it.

Also here: `Function` at `read.c:1546` has the same shape ("only owns length, name, and
prototype") and the same missing check; it is not in the diverged table only because those three
are answered earlier.

Decision: `name` and `message` become data properties on `Error.prototype` and the six subclass
prototypes (the seven `(itself).name` rows come with them), and the two absent sites test the key
before answering.

### 3. A catch-all turned every refusal on a well-known symbol into `undefined` (35 rows)

`@@species` on 20 constructors, `@@toStringTag` on `ArrayBuffer.prototype` and
`DataView.prototype`, `@@toPrimitive` on `Date.prototype` and `Symbol.prototype`,
`@@hasInstance` on `Function.prototype`, `@@match`/`@@matchAll`/`@@replace`/`@@search`/`@@split`
on `RegExp.prototype`, and `Symbol.dispose`/`Symbol.asyncDispose`.

Not a seam hole: the shared table carries `@@` keys, several of these were in the native
backlog, and the backlog refuses -- honestly. The refusal then reached `kiln_c99_get_key`
(`read.c:1651`), which converted *any* refusal on a reserved key into `undefined`, for the
runtime's own probes (`kiln_c99_to_primitive` asking every reference for `@@toPrimitive`) and
program reads alike. The comment that explained it was lost in `062567fb` (2026-09-14); the
original (`500b727c`) said: "THIS IS A CATCH-ALL AND NOT A LIST, WHICH MAKES IT THE ONE ABSENCE
HERE THAT CAN LIE ... The fix is always upstream."

Decision: the seven machine probes get their own tolerant entry (`kiln_c99_probe_symbol`), the
catch-all comes out of the program path, and the members get answered: `@@species` from surface
rows for the table-driven constructors and from the receiver for the rest, the prototype methods
from the runtime bootstrap in TypeScript, the two symbols as well-known rows. The interpreter's
`defines_species` list is the one hand list left; the table now names the same constructors, so
drift between them is a compile error on the native side.

### Not native's fault (8 rows)

- `AggregateError.length`, `SuppressedError.length` (interpreter 1, native 2), `AbortController`,
  `AbortSignal`, `TextEncoder`, `TextDecoder` `.length` (interpreter 1, native 0): **native
  matches node**; the interpreter is wrong.
- `Promise.allKeyed`, `Promise.allSettledKeyed`: a proposal the interpreter published that node
  does not have. Remove from the interpreter or leave native alone.

## Order

1 first: it is two objects, the replacement already exists and is already linked under `--node`,
and it is 52 of the 111 plus every `console.debug` in every program. 2 is an afternoon. 3 is the
one that stops the class from coming back, and is the reason `--gate` should join
`gate-quick` once 1 and 2 have brought the unexplained count to zero -- `SILENT_UNDEFINED_ACCEPTED`
exists for the rows that are right.

## Repro

`probe1.mjs` (the snippet above, with `console.log` of each `typeof`) under
`kiln run` and `kiln --one-shot build --profile native-release`; the two outputs differ on every
line but the last three. `scripts/native-surface-inventory.py --gate` exits non-zero.

## Outcome (2026-09-19)

All three causes landed on `alee/status-check-0918`, plus two things found on the way:

- `Object.defineProperty` on a built-in value refused ("a property definition on a value with
  no property store") while assignment landed in the callable store; definitions now land in the
  same store. A library installing a symbol hook on `Map.prototype` hit this.
- `collect_typed_call_sites` returned nothing when a function had proved no numbers, while the
  use site emits the typed chain for a zero-argument call regardless; the runtime bootstrap's
  `console.clear()` then referenced a callee cache nothing declared. Fixed in `sites.rs`.

Left as they are, and why:

- `TypeError.prototype.name` and the other five subclass prototypes answer `"Error"` natively:
  the native model has one `Error.prototype` for every kind but `AggregateError`, so the store
  can hold one name. Not `undefined` any more; a distinct prototype per kind is its own change.
- `crypto.randomUUID` and the rest refuse by name until `node:crypto` is linked, and that module
  now replaces the global with the full surface. Always linking it costs every binary the SHA
  code; tracked here rather than done.
- `console.log("a %s", "b")` prints `a %s b` natively and `a b` interpreted: native's console
  has no `util.format`. Pre-existing, and `assert`/`trace` inherit it.
- `BigInt64Array^` (5 rows): the interpreter names `%TypedArray%` `"intrinsic"`, which node does
  not; an interpreter question.
- `Promise.allKeyed`/`allSettledKeyed` refuse natively now (backlog rows) rather than reading
  `undefined`; whether the interpreter should carry a proposal node lacks is a separate question.

Measured again at `047bc973` after the fixes (617 native runs): diverged 117 -> 17, silent
`undefined` 111 -> 0 unexplained (`--gate` passes, 5 accepted `BigInt64Array^` rows), refused
299 -> 313, implemented 1304 -> 1391. The 17 diverged are the 5 accepted rows and the twelve
subclass-prototype `name` rows answering "Error". `mise run gate` at `047bc973`: crate tests
1945/1954, native parity, thirdparty 17/17, facts 31/31, node 36/36, regress 15/15, server 6/8
(two skips), Test262 no chunk regression. The thirdparty timing baseline flagged `ms` at
+164% -- measured at a load average of 18, alongside `acorn` and `postcss` reading 33% *faster*;
a clock reading by this repository's own rule, to be re-run on a quiet machine before it is
believed either way.
