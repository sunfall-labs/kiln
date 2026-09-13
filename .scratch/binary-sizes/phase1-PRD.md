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
  distinct names, 35 builtins, computed access, closed world.
- Track A phase 1 landed: `http` / `realm-http` features, defaults
  unchanged (slim rlib: 0 hyper refs vs 3378). Track A phase 2 (packaging +
  selector) running in background.
- Step 3 audit done (see below); implementation next.

## Step 3 design (demand-driven dispatch router)

Audit findings:
- Only direct implementation FUNCTION calls root code; integer code
  comparisons (`calls.c:77`, guards everywhere) root nothing.
- The chain root is `kiln_c99_invoke_method_dispatch` (`dispatch.c:1158`)
  plus grouped fast-paths in `calls.c:97-120` and `read.c:87`.
- Several `NODE_*` codes sit in the always-core `Direct` group
  (`NODE_ASYNC_RESOURCE`, `PROCESS_NEXT_TICK`, `NODE_HTTP_SERVER_RESPONSE`
  at `prelude.rs:926-958`): those implementations stay regardless.
- Droppable groups (no unconditional prelude-internal references):
  `NodeEvent`, `NodeNet`, `NodeServer`, `NodeHttp`, `NodeResponse`,
  `Buffer`, `NodeUtil`, `NodeDiagnostics`, `NodeStream`, `NodePath`.

Design:
- Split the dispatch chain + grouped fast-paths into per-group functions in
  their own sections (mechanical move, same order, same code).
- facts computes the retained group mask from: selected-graph `NodeBuiltin`
  nodes (exact), retained `Builtin` variants, retained `"node:*"` string
  constants (literal dynamic requires); open world / computed-require keeps
  everything (same trust basis as liveness).
- The router itself is generated into the launcher TU (already per-program,
  no new compile unit), calling retained groups in canonical order.
- Bench-gate: router must not regress warm dispatch numbers.
- Init tables (step 4) follow the same pattern afterward.

## Step 3 implementation checklist (v1: node-group router)

Mechanism: per-program group mask → `-D KILN_RETAIN_GROUP_X` flags on the
prelude compile → `#ifdef` guards on dispatch group blocks AND `install_*`
calls in `globals.c`. Mask folds into the prelude cache key. Steps 3+4 share
this mechanism (init pinning makes dispatch-only pruning pointless for
`NodeEvent`/`NodeStream`: `globals.c:97,192-202` installs them on every
`process`/stdio object).

v1 mask rules (final):
- Base CORE always: `Direct ArrayLookup StringDispatch Generic Temporal*
  DateSetter MathNumber` (+ rang-dispatched iterators, no bit).
- `NodeHttp NodeResponse NodeServer NodeNet` += http-family/net imports
  (superset mapping: Http/Https/Http2 ⇒ all four + Stream + Event;
  Net ⇒ Net + Server + Stream + Event). Graph `dependencies` cover CJS
  literal requires; successful builds resolve every require statically, so
  no separate require-bail is needed.
- `NodeEvent NodeStream` += above, or `Builtin::Process` retained, or
  computed access present (only `process` and `globalThis` dynamics can
  reach them without imports; user-object dynamics implicate nothing).
- `Buffer` += buffer import, retained `Builtin::Buffer`, or computed access
  (`globalThis[k]` can yield the `Buffer` global).
- `NodePath NodeUtil NodeDiagnostics` += matching builtin import only.
- Bail to full mask: direct eval, `new Function`, retained `Builtin::Eval`,
  any `dynamic_imports`, `has_dynamic_import`.
- `process.on`-style access needs no name table: it always retains
  `Builtin::Process`, which keeps Event/Stream by rule above.
- FS/crypto/url codes audited `Direct`-only: those builtins need no groups.
  (Re-verify by grepping rows when implementing; any non-`Direct` FS row
  joins the mapping table instead.)

1. facts: `retained_groups(graph, demand, members) -> u32` bitmask.
   Inputs: selected-graph `NodeBuiltin` nodes (exact builtin→groups table,
   conservative supersets e.g. http ⇒ {HTTP,RESPONSE,SERVER,NET,STREAM,EVENT});
   retained `Builtin::Buffer` ⇒ `Buffer` group; demanded event-ish names
   (`on/once/emit/...`, small hand table) ⇒ `NodeEvent` (covers `process.on`
   without imports); retained literal `"node:*"` requires; bail to full mask
   on open world / dynamic import / non-literal require / any CJS with
   unresolvable requires.
2. backend-c: `#ifdef` guards on the 10 node-ish group blocks in
   `dispatch.c`, grouped fast-paths in `calls.c`, `read.c:87` header path,
   and `install_*` calls in `globals.c`. Core groups + `Direct` never guarded.
3. kiln-native: mask → `-D` flags in prelude `CompileOptions` + mask digest
   in `prelude_object_options`/key (single + graph paths; single-program
   path uses full mask — no import table, same rule as provider selector).
4. explain: print retained group list in the Demanded Members section.
5. Tests: mask unit tests (no-node ⇒ core-only; http ⇒ superset; dynamic ⇒
   full); conformance/node + server corpora must pass with router on;
   `mise run bench` before/after (dispatch hot path).
6. Measure with `binary-sizes.sh`; Effect target is observability-only.

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
