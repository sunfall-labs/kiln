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
