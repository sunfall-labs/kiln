# A constructor that calls a static private method loses the instance's fields

**Status:** open, root cause not found · **Severity:** high — latent memory corruption

## The four-line repro

    class C {
      #n = 7;
      static #c = 0;
      static #r(): number { C.#c += 1; return C.#c; }
      constructor() { C.#r(); }        // <- calling a *static private method* is the trigger
      n(): number { return this.#n; }
    }
    console.log(new C().n());
    // kiln: uncaught TypeError: Cannot read private member #n from an object whose class
    //       did not declare it

Native backend only; the interpreter is correct, so it surfaces as a parity divergence.

**It does not reproduce on today's `main`.** It needs the object layout perturbed — see below.
That is what makes it dangerous: it is latent, and the next field added to `kiln_object` wakes it.

## What is established

Instrumenting `kiln_c99_own_private` to dump the receiver at lookup:

    PROBE lookup [@@private:0:r] tag=5                    <- static method, on the constructor: fine
    PROBE lookup [@@private:0:n] tag=9 count=0 keys:      <- the instance has NO own properties

`new C()` yields a *fresh, empty* object rather than the initialised instance. Not the wrong
receiver -- tag 9 is `KILN_VALUE_OBJECT` -- and not a missing single slot: every field is gone.

Ruled out, each by experiment:

* **Not the collector.** `KILN_GC=0` still fails. (Collection also only runs at the top of the
  drive loop with no generated frame live -- ADR 0041.)
* **Not key interning.** `KILN_KEYS_CHECK=1` does not trip; the interned-key invariant holds and
  the slot is absent by bytes as well as by pointer.
* **Not caching.** Reproduces with `.kiln/bin` and `.kiln/cache` both removed.
* **Not a build profile.** Reproduces in debug and release once caches are clear.
* **Not the struct's size.** Appending 32 bytes (`double[4]`) at the end changes nothing.
* **Not a duplicate declaration.** One `typedef struct kiln_object`, no positional initialiser.
* **Not stale prelude artifacts.** `NATIVE_PRELUDE_VERSION` is the hand-maintained integer that
  names cached prelude artifacts (`kiln-runtime-prelude-<N>`), and it looked like the culprit --
  change the prelude without bumping it and old objects stay "valid". Bumping it to 3 alongside
  the perturbation does **not** fix the failure, so reuse is not what is happening. (The constant
  is still worth deriving from a hash of the prelude source rather than maintaining by hand; it is
  a real drift hazard, just not this bug.)

What narrows it:

| constructor body | result |
| --- | --- |
| `C.#c += 1` — touches a static private *field* | works |
| `Math.max(1, 2)` — an ordinary call | works |
| `C.#r()` — calls a static private *method* | instance loses every field |

So the trigger is specifically **invoking a static private method during construction**, and the
symptom is the constructed instance being replaced by an empty object.

## How to wake it

Insert a field into the *middle* of `kiln_object` (after `arguments_brand`), initialise it in
`kiln_c99_new_object`, change nothing else, rebuild, clear `.kiln/bin` and `.kiln/cache`.

Appending at the end does not wake it. Since size alone is harmless and only *offsets moving*
matters, the most likely shape is a write through a stale or mis-sized view of the object -- i.e.
memory corruption whose blast radius depends on what now sits at the shifted offsets.

## Why this blocks work

`error_brand` is appended at the end of `kiln_object` rather than placed beside `arguments_brand`
where it belongs, with a comment pointing here. That placement is a workaround, not a fix: the
corruption is still there, and the next person to insert a field pays for it -- with a failure
message about private members that names nothing near the cause.

## The sanitizer run: clean, which rules out the obvious cause

`cargo run -p kiln-backend-c --example dump_prelude` writes the assembled prelude; the module unit
comes from `KILN_NATIVE_KEEP_WORK=1`. Both were compiled with
`-fsanitize=address,undefined -g` and linked against the run's own `launcher.o`, `runtime.o` and
`libkiln_native_number_provider.a`.

**The sanitized build reproduces the failure and reports nothing.** So it is not an out-of-bounds
or wrong-width write in the emitted C or the prelude -- which was the leading hypothesis and is now
dead. Caveat worth keeping: `runtime.o` and `launcher.o` are prebuilt and were *not* instrumented,
so a bad write originating in those is still possible and would be invisible here.

## Layout, measured

`clang -Xclang -fdump-record-layouts`, with the probe field inserted after `arguments_brand`:

    52 | int arguments_brand
    56 | int kiln_probe
    64 | size_t accessor_count      <- was 56
    72 | int read_only_seen
    76 | int error_brand
       | [sizeof=80, align=8]

Appending 32 bytes at the end also changes `sizeof` and is harmless, so size is not the trigger:
what matters is `accessor_count` and `read_only_seen` moving. Those two are read by
`kiln_c99_put_mode`'s prototype-chain skip, which is the place to look next.

## Also ruled out

* **No Rust mirror of the layout.** Nothing in `kiln-native-runtime` or `kiln-runtime` names
  `accessor_count`, `arguments_brand` or `module_namespace`, so `runtime.o` does not carry a
  second view of the struct.

## Next step for whoever picks this up

Instrument `kiln_c99_put_mode`'s use of `accessor_count`/`read_only_seen` under the perturbation,
and trace the `new` sequence for a constructor that calls a static private method. The emitted
module unit writes `kiln_property` slots directly in at least one place (the ESM-namespace
builder), so an audit of every module-side direct write against the current field list is worth
doing on its own account -- a field added to `kiln_property` or `kiln_object` that such a path
does not initialise is the same class of latent bug.

## Three confounds that cost hours here

1. `.kiln/bin` caches *prebuilt native executables* and is not invalidated by a prelude change.
   `kiln cache gc` does not clear it (it evicted 2584 of 2884 records and kept the relevant one).
2. `scripts/native.sh` runs `target/debug/kiln`; manual checks used `target/release/kiln`. Sharing
   one artifact cache made a layout bug look debug-only.
3. A `git stash` bisect "proved" the change at fault, because stashing forced the rebuild that
   cleared the stale artifact.

Clear `.kiln/bin` **and** `.kiln/cache`, and compare at equal build freshness, before believing any
native measurement taken across a prelude change.
