# PRD: Phase 1 — floor diet (demand-driven prelude)

## Key finding (design driver)

Release builds already compile every TU with `-ffunction-sections
-fdata-sections` and link `--gc-sections` (`kiln-native-toolchain/src/lib.rs`).
The 17MB Effect binary is *after* linker GC — the prelude is fully rooted, so
splitting translation units alone would save ~nothing. Phase 1 is de-rooting,
not splitting.

Roots that keep everything (all in the single concatenated prelude TU):
- `kiln_c99_invoke_method_dispatch` (`runtime/dispatch.c:1158`): one function
  whose if-chain/switch references every builtin implementation (~116KB of
  dispatch rooting megabytes of implementations).
- Global init tables (`runtime/globals.c`, `surface.c`, `methods.c`): eager
  installation references every implementation. Name strings are cheap and
  stay; implementations are the cost.
- Rust provider archive ships `realm` (Tokio/Hyper) in every binary
  (`kiln-native-number-ffi`: `default = ["realm"]`); archive granularity
  decides what the linker can drop. Measurement pending.

## Status

- Step 1 landed: prelude bytes + cache-hit in report (`PROTOCOL_VERSION` 29).
- Step 2 landed: `analyze_demanded_members` — Effect hello shows 787
  distinct names, 35 builtins, computed access, closed world. The computed
  flag means the router (step 3) needs receiver-type joins, not just names.
- Track A phase 1 landed: `http` / `realm-http` features, defaults
  unchanged (slim rlib: 0 hyper refs vs 3378). Caveat: slim FFI config still
  unifies `kiln-runtime/http` on via defaults — packaging must disable
  defaults or invert them; no link-time enforcement yet.

## Plan (remaining)
2. **Retained method-code set (2–3d):** facts analysis over retained IR
   listing every `KILN_C99_METHOD_*` code the executable can invoke, plus
   entry/bootstrap/launcher needs. Unit-tested by comparing against codes
   present in emitted C for the Effect probe.
3. **Demand-driven dispatch (1wk):** split the giant dispatch into per-group
   functions in their own sections + a generated per-program router over the
   retained set. Unreferenced implementations become unreachable and existing
   `--gc-sections` drops them. **Perf risk:** dispatch is the hottest path
   (see group-test ordering comments in `dispatch.c`); router must preserve
   test order. Gate with `mise run bench` before/after.
4. **Demand-driven init (3–4d):** generated init installs implementations for
   retained builtins only; name tables stay complete so reflection
   (`getOwnPropertyNames`, `typeof`) is unchanged. Open world (eval/dynamic
   import) keeps everything — same trust basis as existing liveness.
5. **Rust archive (track, decide on numbers):** feature-split the provider
   (HTTP engine optional) if measurements justify; touches packaging +
   manifest versioning, so separate commit.

## Out of scope

App-code shaking (Phase 2/3), TU splitting (no value), dev-profile GC
(diagnostics profiles keep full layout).

## Acceptance

- `binary-sizes.sh --check` drops on all three targets; Effect hello drops
  (target: 17MB → ~13MB with dispatch+init done, floor ~7.4MB → ~5MB).
- `mise run bench`: no dispatch regression (compare.sh warm numbers).
- Full `gate` green (soundness: differential corpus + Test262 chunk parity).
