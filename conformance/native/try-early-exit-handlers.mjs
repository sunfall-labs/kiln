// Leaving a `try` early -- `continue`, labeled `continue`, `break`, `return`, through `finally` --
// retires only that path's handler: code after the exit in the same `try` body is still protected.
// The native graph once computed handlers in code order, so the `PopHandler` a `continue` jumps
// past unprotected the rest of the body and its next throw escaped every catch.
const out = [];
let i = 0;
while (i < 3) {
  i++;
  try {
    if (i === 1) { out.push("first"); continue; }
    throw new Error("boom " + i);
  } catch (error) { out.push("caught " + error.message); }
}
function viaReturn(k) { try { if (k) return "returned"; } catch {} return "fell"; }
viaReturn(true);
try { throw new Error("after return"); } catch (e) { out.push("caught " + e.message); }
for (let k = 0; k < 2; k++) { try { if (k === 0) break; } catch {} }
try { throw new Error("after break"); } catch (e) { out.push("caught " + e.message); }
outer: for (const a of [1, 2]) { for (const b of [1, 2]) { try { if (b === 1) continue outer; } catch {} } }
try { throw new Error("after labeled continue"); } catch (e) { out.push("caught " + e.message); }
for (const k of [1, 2]) { try { try { if (k === 1) continue; } finally { out.push("finally " + k); } throw new Error("inner " + k); } catch (e) { out.push("caught " + e.message); } }
function deep() { for (;;) { try { try { return 1; } catch {} } catch {} } }
deep();
try { throw new Error("after nested return"); } catch (e) { out.push("caught " + e.message); }
function noCatch() { for (const k of [1, 2]) { try { if (k === 1) continue; throw new Error("unhandled? " + k); } catch (e) { return "inner caught " + e.message; } } }
out.push(noCatch());
console.log(out.join(" | "));
