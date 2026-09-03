# ADR 0042: `Intl` is present and refused by name in the native backend

## Status

Accepted.

`typeof Intl` now answers `"object"` under `--native`, as it does in the interpreter, and every
member behind it refuses by name. Linking ICU4X into the provider archive is deferred, not
rejected on principle; this records the measurement and the rule that made deferring the answer.

## Context

`typeof X === "undefined"` is how real code decides whether a feature exists. A compiled program
that reads `undefined` where the interpreter reads `object` does not fail — it silently runs the
branch its author wrote for a realm that never had the feature. That is a wrong answer, and a
wrong answer is the one thing this backend is not allowed to give. `Intl` was in exactly that
state: an object in the interpreter, absent natively, with nothing in
`conformance/native` asking.

There are three ways to stop reading `undefined`, and only two of them are honest.

1. **Link the engine.** ADR 0025 says language semantics that need an engine are linked rather
   than reimplemented, and ADR 0029 §4 names focused ICU4X crates as the candidate when Kiln
   begins `Intl` coverage. The interpreter already does this: `kiln-intl` links
   `icu_decimal`, `icu_plurals`, `icu_list`, `icu_collator`, `icu_segmenter`, `icu_datetime`,
   `icu_time`, `icu_calendar` and `icu_experimental`, and both engines are supposed to consume the
   same one.

2. **Reimplement enough of it.** An `Intl.NumberFormat` that groups by threes and formats `en-US`
   is a few hundred lines and would make the common case print. It is also precisely what ADR 0025
   exists to forbid, and it fails in the direction that matters: a program asking for `de-DE` gets
   a plausible string that is wrong, and nothing announces it.

3. **Publish the name and refuse behind it.** `typeof Intl` is `"object"`, `Intl.NumberFormat`
   stops the program and says `GetProperty(NumberFormat)`.

## Decision

**Publish `Intl` on the native global object as a namespace whose every member refuses by name.**
Do not link ICU4X into the provider archive yet.

Option 2 is excluded outright by ADR 0025 and is not a cost question. The choice is between 1 and
3, and it turns on what linking costs every artifact.

### What it costs, measured

`libkiln_intl.a` — `kiln-intl` built as a release staticlib for `aarch64-apple-darwin`, the target
the native backend ships for — is **66.5 MiB** of object code. The whole provider archive that
carries regular expressions, BigInt and `Temporal` today is **80.3 MiB**. So the internationalization
data is comparable in size to every linked language engine Kiln has, put together.

A staticlib is pre-link and the linker discards a great deal of it. An earlier measurement on this
repository put the surviving cost at **+23 MiB on every native artifact**, and — this is the part
that decides it — found the cost close to flat in how many formatters a program uses, because
ICU4X's `compiled_data` bakes one data bundle per data key rather than per call site. A program
that formats one number pays for the collator, the segmenter and the calendar too. *That figure is
carried forward from the earlier measurement and was not re-measured here; the 66.5 MiB and 80.3 MiB
above were.*

### Why that number decides it

ADR 0029's adoption rule 3 requires that only required features be enabled and that crates on the
rebuild path obey ADR 0004's budget. Neither survives a bundle that cannot be subset: there is no
"only required features" configuration of `compiled_data`, so the rule cannot be satisfied by
tuning — it can only be satisfied by not linking it yet, or by moving the data off the artifact
entirely.

ADR 0004's correction is that a dependency's cost is where it sits, not that dependencies are
expensive. A 23 MiB flat addition to every emitted artifact sits on the path of every native
build, every artifact-cache entry and every `kiln run --native` that misses the cache — for a
feature most programs do not call.

And the thing being bought is smaller than it looks. Refusing is not the same as being absent: a
program that feature-detects `Intl` and falls back now takes the fallback in both engines, and a
program that uses `Intl` unconditionally stops with a message naming the member. Neither is a
wrong answer. What linking buys is turning a refusal into an answer — real, and not worth 23 MiB
on every artifact before anything asks for it.

### What would reverse this

- A demand: a probe, a corpus entry or a user program that needs `Intl` natively rather than
  needing it to be honestly absent.
- A data path that is not per-artifact — `icu_provider`'s blob provider with the data loaded from
  a file beside the binary, or baked per data key actually reached. Either would make the cost
  proportional to use, and rule 3 satisfiable, which is what fails today.

Until one of those, `Intl` is a name that tells the truth about itself.

## Consequences

- `typeof Intl` is `"object"` in both engines; `scripts/native.sh` compares them and
  `conformance/native/global-surface.ts` fails if they ever diverge again.
- The `Intl` entry in the native refusal tally is a work queue entry, not a defect.
- `kiln-intl` stays interpreter-only. It is not on the native artifact's link line, so ADR 0004's
  budget is unaffected.
- The same reasoning applies unchanged to the other globals published-and-refused alongside it:
  `DataView`, `SharedArrayBuffer`, `WeakRef`, `FinalizationRegistry`, `ShadowRealm`, `Iterator`,
  the three wide typed arrays, the disposable stacks, `SuppressedError`, `EvalError`,
  `TextDecoder`, `eval` and `performance`. None of those is a size question — they are simply not
  written yet — but all of them are now names that refuse rather than names that read `undefined`.
- `DataView` left that list on 2026-09-02: it is a native constructor now, with a builtin code
  outside the published-and-refused range, and the code it vacated is retired rather than reused
  so that every code inside that range still means what this ADR says it means. The reasoning is
  unchanged for the rest of the list — and it is what `DataView.prototype.getFloat16` and
  `setFloat16` now do, being the two members of an implemented built-in that are still missing.
