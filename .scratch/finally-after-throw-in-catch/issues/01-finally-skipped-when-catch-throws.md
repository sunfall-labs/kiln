# `finally` does not run when its `catch` block throws

Found 2026-09-05 while probing the graph emitter's handler lowering. Both engines agree with each
other and disagree with node, so this is in `kiln-ir`'s lowering of `try`/`catch`/`finally`, not
in either backend.

```js
function nested() {
  try {
    try { throw 1; } catch (e) { throw e + 1; } finally { console.log("inner-finally"); }
  } catch (e) { return "outer:" + e; }
}
console.log(nested());
```

node prints `inner-finally` then `outer:2`. kiln, interpreted and native alike, prints only
`outer:2`: the throw inside the catch block propagates to the outer catch without running the
inner `finally`. A throw from the *try* body does run it (`fin()` in the same probe), so the
finally handler is registered for the try body but not kept active across the catch body.

Probe: the `nested` function in the handler probe written for the graph emitter; it belongs in
`conformance/differential` once fixed (adding it now would fail the gate on a known bug).

Triage: bug, language semantics, both engines. The fix is in the compiler's handler layout: the
catch body must remain protected by the finally handler until the finally runs.

Status: resolved · Severity: high
- In `crates/kiln-ir/src/compile.rs` (`try_statement`):
  - When `has_finalizer` is true and a `catch` handler is present, emit a nested `PushHandler` around the catch clause's body.
  - If the catch clause's body finishes normally, pop the handler and emit the normal finally block.
  - If the catch clause's body throws, jump to a dedicated catch-exception landing pad: execute the finally block, and (if the finally block does not terminate with `return` or `throw`) re-throw the catch exception outwards to enclosing handlers.
  - Added `statement_always_terminates` / `statements_always_terminate` to avoid emitting dead `Throw` instructions when `finally` unconditionally returns or throws.
- Added differential conformance test: `conformance/differential/finally-after-throw-in-catch.js`.
- Added native exception test in `conformance/native/exceptions.ts`.
- Verified clean run on `scripts/differential.sh`, `scripts/native.sh`, and `gate-quick`.
