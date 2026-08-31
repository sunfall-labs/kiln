// An error's `name` belongs to its prototype, not to the instance.
//
// An own `name` on the instance shadows everything a subclass says about itself: a
// `name = "TimeoutError"` class field is written during construction and was then overwritten,
// and a `get name()` on the subclass prototype was never reached at all. Both engines had the
// own property, so both answered "Error" for an error the program had explicitly named -- which
// is what makes `catch (e) { if (e.name === "TimeoutError") }` take the wrong branch.
//
// The own key list is asserted rather than just the value: the value is what a program reads,
// and the key list is what tells the two engines apart from an engine that merely got lucky.
const e = new Error();
console.log("own-empty", JSON.stringify(Object.getOwnPropertyNames(e)));
const m = new Error("m");
console.log("own-msg", JSON.stringify(Object.getOwnPropertyNames(m)), m.message, m.name);
console.log("hasOwn-name", Object.hasOwn(m, "name"));
class B extends Error { name = "B"; constructor() { super("b"); } }
const b = new B();
console.log("B", JSON.stringify(Object.getOwnPropertyNames(b)), b.name, b.message, String(b));
class A extends Error { name = "A"; }
console.log("A", new A("a").name, String(new A("a")));
const t = new TypeError("t");
console.log("TypeError", JSON.stringify(Object.getOwnPropertyNames(t)), t.name, String(t));
console.log("empty-message", JSON.stringify(e.message), e.name, String(e));
class G extends Error { get name() { return "G"; } }
console.log("G", new G("m").name, String(new G("m")));
try { null.x; } catch (err) { console.log("thrown", err.name, err instanceof TypeError); }
