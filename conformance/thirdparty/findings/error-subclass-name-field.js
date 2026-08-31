// A class field named `name` on an Error subclass is overwritten with "Error".
//
// This is p-timeout's `TimeoutError` exactly, and it is what makes `catch (e) { if (e.name ===
// "TimeoutError") ... }` -- the standard way a server distinguishes a timeout from a real failure
// -- take the wrong branch. The field survives when the subclass has no explicit constructor (A),
// so whatever installs the own `name` runs after the field initialisers only in the explicit-
// constructor case. Assigning `this.name` in the body (D) still works, which is why this hides.
//
// node:  A name=A  B name=B  C name=C  D name=D  E name=E
// kiln:  A name=A  B name=Error  C name=Error  D name=D  E name=Error
class A extends Error { name = "A"; }
class B extends Error { name = "B"; constructor() { super(); } }
class C extends Error { name = "C"; constructor(m) { super(m); } }
class D extends Error { constructor(m) { super(m); this.name = "D"; } }
class E extends Error { name = "E"; constructor(m) { super(m); this.tag = 1; } }
for (const [k, V] of [["A", A], ["B", B], ["C", C], ["D", D], ["E", E]]) {
  const e = new V("m");
  console.log(k, "name=" + e.name, "msg=" + JSON.stringify(e.message));
}
// The accessor form loses the same way, and to the inherited Error.prototype.name.
class G extends Error { get name() { return "G"; } }
console.log("G", new G("m").name);
