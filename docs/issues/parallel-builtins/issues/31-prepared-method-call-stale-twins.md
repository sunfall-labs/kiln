# 31. Prepared method calls must prove typed twins

Status: resolved

Two adjacent release-only failures in prepared method calls shared one representation mistake:
the emitter treated a live integer/number twin as proof that the twin contained the current
boxed value. Liveness only says a fast representation may be read later; the graph's type proof
must also establish that representation.

The first failure was a post-loop method argument. Induction-variable analysis placed the slice
index in `live_ints`, but `ints` did not prove an integer twin for that Phi. The prepared call
passed stale `iN` (zero) instead of current boxed `vN` (two), so Effect `Array.dropWhile([1, 2])`
returned `[1, 2]` rather than `[]`. Both prepared-call argument forms now select `iN` only when
both `ints` and `live_ints` contain the value.

The second failure was a typed method result used by a speculative caller. When a receiver had no
numeric row, the call correctly ran the boxed method body once, but the caller copied that result's
tagged bits into its numeric twin. A method returning `"y"` therefore became `NaN`, and
`o.val() + 1` produced `NaN` instead of `"y1"`. Typed-entry success now marks its numeric twin as
already materialized. At speculation-marked prepared calls, a boxed fallback result is checked
before it is copied; a non-number continues at the generic resume point after the call, so side
effects are not repeated.

The regression coverage uses the existing
[`release-typed-method-receiver-without-row.js`](../../../../conformance/native/release-typed-method-receiver-without-row.js)
probe, extended with per-receiver call counts. The pre-fix frozen CLI prints `[3, NaN]` twice for
the mixed map and shows `[1, 1]` calls; the fixed CLI prints `[3, 'y1']` twice with the same
`[1, 1]` calls. Ordinary and `KILN_GC_STRESS=1` native parity both match. The new
[`prepared-loop-phi-method-arguments.ts`](../../../../conformance/native/prepared-loop-phi-method-arguments.ts)
probe separately covers the stale post-loop index and `dropWhileFilter` result.

The pinned direct-TypeScript Effect run on the final two code fixes passed **599/599 native
cases** across Array (146), Chunk (105), ConfigProvider (101), Graph (139), HashMap (57), and
Optic (51), with no skips, failures, timeouts, inventory mismatches, or Node/Vitest outcome
mismatches. The pinned source is Effect commit `67ba4e46a11ccda0b6761578bfd22c04ae00167d`.
The runner report and raw per-file build/run logs are committed under
[`evidence/prepared-method-call-2026-10-04`](../evidence/prepared-method-call-2026-10-04/);
[`SHA256SUMS`](../evidence/prepared-method-call-2026-10-04/SHA256SUMS) records their digests.

The measured CLI was `native-release`, SHA-256
`f8d759a7155451110a78a108954f26af5e6d3c3e3feff256c3bbbe0a3b487587`. Its provider archive
SHA-256 is `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d`, manifest
SHA-256 `b03c48a204de3f911b424ce62b7d96a88ef595de0f99217cba8c34caf6c5dd6e` (ABI 82).

Fix commits: `5419e417` (argument representation proof), `357ff2c7` (typed-result fallback), and
`05d6c596` (exactly-once regression coverage).
