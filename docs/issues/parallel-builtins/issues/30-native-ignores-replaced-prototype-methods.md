# 30. Native built-in method calls ignore replaced prototype methods, a UTF-16 `split` limit and UTF-16 `parseInt`

Status: resolved
Scope: general node parity, found by issue 09's probes; outside the concurrency and parallelism plans.
Found: 2026-09-27, on `work/parallel-tier4` and on `origin/main` (bc06ed29) alike; every item is in
the *sequential* native path -- the interpreter answers node's.

1. **A replaced built-in method is never called.** After `String.prototype.trim = f`,
   `" z ".trim()` still runs the built-in natively (dev and release), and
   `" z ".trim === String.prototype.trim` is `false`; the same for `Array.prototype.join` and
   `Number.prototype.toFixed`. The interpreter calls `f`. (The parallel sites are unaffected: the
   store is recorded in the realm's modified-built-ins record, and every pooled lowering stands
   down -- `conformance/native/parallel-pure-refused.js` checks only that parallel and sequential
   agree, since the sequential answer itself differs from the interpreter here.)
2. **`split` on a UTF-16 string ignores its limit** (below the number of pieces):
   `"日本,語,27".split(",", 1)` answers three pieces natively, one in the interpreter; `0` answers
   the pieces rather than `[]`. The UTF-16 path goes through the regexp split, which does not read
   the limit.
3. **`parseInt`/`parseFloat` of a UTF-16 string** fail with `kiln: out of memory`:
   `kiln_c99_string_ascii_bytes` answers NULL for UTF-16 and the dispatch reports that as
   `KILN_C99_OUT_OF_MEMORY` (`dispatch_part_a.c`, `KILN_C99_METHOD_PARSE_INT`).
4. **A native recursion of 5,000 frames** throws `RangeError: Maximum call stack size exceeded`
   (`KILN_C99_MAX_CALL_DEPTH` 2048) where the interpreter and node finish.

## Acceptance

- A `conformance/native` probe per item; both engines match node.

## Comments

### 2026-09-28 -- interpreter oracle stack guard restored

The existing `deep-recursion-release.js` probe stopped `scripts/native.sh` before native
comparison: the interpreter's `viaCall(150)` exhausted a 16 MiB program thread. Its method
dispatch called `call_value`, nesting another Rust VM loop for every
`Function.prototype.call`. The instruction dispatcher now forwards `call` and `apply` into the
existing VM loop, so both 150-deep probes finish. A separate nested `map` callback still genuinely
reenters Rust; it aborted at 125 levels before the old native-depth limit of 192. The limit is
now 64, which makes that path throw a catchable `RangeError` with room for different native
frames. `conformance/regress/r48-native-callback-depth.js` checks both paths and a call after the
caught error. Interpreter unit tests passed 203/203; regress passed 48/48; full native parity
passed 349 matches, one known refusal and zero divergences.

2026-09-27, branch `work/patched-methods` (d8d87d74, 3adcbe27, dc06b7b5).

**1. A replaced method (native runtime, emitter; interpreter).** Three places answered a
receiver kind's table method without asking whether the program had written that member:
(a) the read, `kiln_c99_get_own_kind` (`read.c`), consulted each kind's table before its
prototype's store -- now, once the realm's *program* has written onto that prototype (one compare
on `kiln_c99_builtin_store_epoch` while no realm has), the store comes first
(`KILN_C99_WRITTEN_MEMBER_FIRST`), and Object.prototype's store likewise ahead of the members the
tables hand out for it; (b) the receiver-kind site cache, `kiln_c99_cached_method_matches`, hit
unconditionally -- it now asks `kiln_c99_tag_method_pristine(tag, code)` (object-granular, then by
the method's name, remembered per code until the next built-in store); (c) the emitted fast paths
in `call_method.rs` -- all prototype-method intrinsics now sit behind one
`kiln_c99_kind_member_unmodified("name")` (no kind prototype holds that name as a program-written
member; remembered per name), and `JSON.stringify`/`parse` behind their member guards. The
typed-array site loops and the parallel family already asked `kiln_c99_builtins_unmodified()`
(whole realm) and the array loops `ARRAY_METHOD_GUARD`; left as they were. Found on the way and
fixed: primitives never reached Object.prototype (`typeof "x".isPrototypeOf` was `undefined`);
`delete` of a member written onto a built-in was a no-op (now removes it from the store, and moves
the realm's store serial); the interpreter answered `re.test`/`exec`/`toString` and every promise
member ahead of the prototype's store (both through `table_step` now).

**2. `split` limit.** The regexp path (`kiln_c99_string_pattern`) and the UTF-16 string-separator
path both found every piece and ignored the limit; both now keep the first `ToUint32(limit)`
pieces (`kiln_c99_split_limit`), which is exactly where the specification's walk stops. This was
also wrong for a *Latin1* subject with a regexp separator (`"a,b,c".split(/,/, 1)`).

**3. `parseInt`/`parseFloat` of UTF-16.** The dispatch read `kiln_c99_string_ascii_bytes`'s NULL
as out of memory. A UTF-16 subject is now read as its leading white space (Unicode's included)
and the ASCII run after it -- every unit either grammar consumes is ASCII. The interpreter trimmed
with Rust's `White_Space` (no U+FEFF): `parseInt("﻿12")` was NaN there; it now uses
`is_js_whitespace`. The pool's pure `parseInt` still declines a UTF-16 subject to the sequential
path, which is now right.

**4. Deep recursion -- chosen: the interpreter's count, the stack to hold it, and a stack floor.**
`KILN_C99_MAX_CALL_DEPTH` is 10,000 (the interpreter's `max_call_depth`; node's default stack holds
about that many simple frames -- node 26 here overflows `f(9990)` itself). A count alone is not
safe: a frame is as big as the C the call passes through, and a dev-build call through the
dispatcher (`f.call`, a `map` callback) costs >13 KiB of stack per level. So (a) every thread a
realm runs on has 64 MiB, what a `worker_threads` realm already had: the launcher's main thread
(Linux raises the soft `RLIMIT_STACK`, which bounds on-demand main-stack growth, never past the
hard limit; macOS links `-Wl,-stack_size,0x4000000`) and `kiln_realm_run_pinned`'s realms (they had
Rust's 2 MiB); the parallel pool keeps Rayon's default (a pure callback recursed 9,063 deep there,
and past 10,000 it refuses with the RangeError as before); and (b) the runtime's call entries
(`kiln_c99_call`, `kiln_c99_invoke_closure`, `kiln_c99_val_invoke_cached_method`,
`kiln_c99_invoke_method`) compare the stack pointer with a per-thread floor (an eighth of the
stack, at least 256 KiB, found once) and throw the same RangeError instead of segfaulting.
Measured with soft 8 MiB / hard unlimited: 9,990-deep plain, method-chain and heavy-frame
recursion finish in dev and release (release also 4,995 deep through `.call` and 3,330 through a
`map` callback); a dev recursion that deep through `.call` or a `map` callback stops with the
RangeError instead of the segfault it was.

**Probes** (`conformance/native/`): `replaced-prototype-methods.js` (dev),
`replaced-prototype-methods-kinds.js` (dev), `replaced-prototype-methods-typed.ts` (release),
`split-limit-utf16.js`, `parse-number-utf16.js`, `deep-recursion.js` (dev),
`deep-recursion-release.js` (release). Each prints node's output in both engines (the `.ts` one
compared against `node --experimental-strip-types`); every one also matched under
`--profile native-release`.

**Verified** (once, final tree): `mbx test -p kiln-backend-c` 215 + 36 passed (ABI snapshot
re-recorded; prelude size band 58,300 -> 59,000 lines, 58,650 measured; `C_STRING_LENGTH_SITES`
74); `-p kiln-native` 232 passed; `-p kiln-interpreter -p kiln-runtime -p kiln-native-toolchain
-p kiln-native-provider-ffi` all passed. `scripts/native.sh`: 310 matched, 1 refused
(shadow-realm, as before), 0 diverged. `scripts/native-realms.sh` N = 1, 2, 8 on the seven probes
plus the issue-20 probes, builtin-intrinsics-see-overrides, string-methods and array-methods: 13
matched.

**Instructions** (`perf stat -e instructions:u`, fixed-iteration programs, base 80dfa03b):

| program | profile | base | this branch |
| --- | --- | --- | --- |
| class-poly, 20M `dist()` | release | 1,802,403,436 | 1,802,514,928 (+0.01%) |
| same, `JSON.foo = 1` first | release | 2,502,441,995 | 2,502,588,339 (+0.01%) |
| string-ops, 400k rounds | release | 1,520,882,933 | 1,520,265,203 (-0.04%) |
| method calls by receiver kind, 2M | release | 42,878,354,710 | 43,088,754,015 (+0.49%) |
| string-ops after `Array.prototype.last = f` | release | 3,420,805,499 | 3,617,670,312 (+5.75%) |
| class-poly | dev | 73,536,759,761 | 74,456,984,857 (+1.25%) |
| string-ops | dev | 10,101,413,068 | 10,254,008,556 (+1.51%) |

**Left / risks.**
- Deleting a *table* member (`delete String.prototype.trim` with nothing written there) still
  leaves it callable natively; the tables are shared and there is no per-realm tombstone.
- A realm that wrote onto a kind's prototype pays the guards' cold halves on every prototype-method
  call (+5.75% on string-ops with an unrelated `Array.prototype` polyfill, where base already ran
  2.25x slower than pristine because `ARRAY_METHOD_GUARD` is object-granular); making the array
  loops' guard member-granular would recover most of that.
- Direct (emitted) recursion does not ask the stack floor, only the count: with a *hard* stack
  limit below 64 MiB (a container's `ulimit -s`), a dev build of a large-framed direct recursion
  can still exhaust the stack before 10,000 calls. Dev builds cost +1-1.5% from the runtime
  entries' floor check (-O0 keeps the helpers out of line); release is flat.
- macOS stack flag is untested here (Linux machine).
- The interpreter bounds native re-entry (`f.call` recursion) at 192, so the `.call` probe
  recurses 150 deep.
