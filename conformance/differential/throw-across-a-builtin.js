// A throw out of a callback that a *built-in* is driving. `map`, `sort`, `forEach` and a promise
// reaction all re-enter the interpreter from inside a Rust frame of their own, and a handler
// outside that frame cannot be jumped to directly: the driver would still be sitting on the
// stack and would carry on running whatever the jump landed on, at the wrong instruction, in the
// wrong frame. The symptom was not an error -- it was the *next* function returning a leftover
// value, several statements later, which is why this probe checks what comes after.
try {
  [1, 2, 3].map((x) => { if (x === 2) { throw "in map"; } return x; });
} catch (e) {
  console.log("caught", e);
}
console.log("after map", 1 + 1);

function boom() { throw "x"; }
function depth() { try { return 1 + boom() + 100; } catch (e) { return 5; } }
console.log(depth());

try { [3, 1, 2].sort(() => { throw "in sort"; }); } catch (e) { console.log("caught", e); }
console.log(depth());

try { [1, 2].forEach(() => { throw "in forEach"; }); } catch (e) { console.log("caught", e); }
try { [1, 2].filter(() => { throw "in filter"; }); } catch (e) { console.log("caught", e); }
try { [1, 2].reduce(() => { throw "in reduce"; }, 0); } catch (e) { console.log("caught", e); }
try { [1, 2].find(() => { throw "in find"; }); } catch (e) { console.log("caught", e); }
console.log(depth(), [4, 5].map((x) => x * 2));

// Nested: a throw from a callback inside a callback, caught between the two.
try {
  [1].map(() => {
    try { [2].map(() => { throw "inner"; }); } catch (e) { console.log("inner caught", e); }
    throw "outer";
  });
} catch (e) {
  console.log("outer caught", e);
}
console.log(depth());

// A callback that throws past a handler *inside* the callback -- which must still work normally.
console.log([1, 2].map((x) => { try { throw x; } catch (e) { return e * 10; } }));
