// A derived class initialises its instance fields when `super()` returns, not when its
// constructor is entered.
//
// The difference is invisible while the parent is an ordinary class -- `this` is the same object
// either way -- and decisive the moment the parent is an exotic builtin, because `super()` then
// hands back a *different* object and everything written before it is abandoned. That is what
// made `class E extends Error { name = "E"; constructor(m) { super(m); } }` read `"Error"`:
// the field was written onto the instance the derived class allocated, and the error the parent
// built replaced it.
//
// The ordering assertions below are the other half. Fields run after the parent's constructor
// and before the derived body, private methods are installed before any field initialiser that
// might call one, and `super()` written inside an arrow still initialises the fields.
class Base { constructor() { this.order = ["base"]; } }
class D1 extends Base { f = (this.order.push("field"), 1); constructor() { super(); this.order.push("body"); } }
console.log("order", new D1().order.join(","), new D1().f);

class D2 extends Base { #p = 5; m() { return this.#p; } constructor() { super(); } }
console.log("private", new D2().m());

class D3 extends Base { #q() { return 7; } r = this.#q(); constructor() { super(); } }
console.log("private-method-before-field", new D3().r);

class D4 extends Array { n = 3; constructor() { super(1, 2); } }
const d4 = new D4();
console.log("array", d4.length, d4.n, Array.isArray(d4));

class D5 extends Base { z = 9; constructor(...a) { super(...a); } }
console.log("spread-super", new D5().z);

class D6 extends Base { y = 1; constructor(flag) { if (flag) { super(); } else { super(); } } }
console.log("two-super-sites", new D6(true).y, new D6(false).y);

class D7 extends Base { w = 2; constructor() { const go = () => super(); go(); } }
console.log("super-in-arrow", new D7().w);

class Base2 {}
class D8 extends Base2 { a = 1; b = this.a + 1; }
console.log("implicit-ctor", new D8().b);

class D9 extends Map { k = "v"; constructor() { super([["a", 1]]); } }
console.log("map", new D9().get("a"), new D9().k);

class Outer extends Base { o = 1; constructor() { super(); class Inner extends Base { i = 2; constructor() { super(); } } this.inner = new Inner().i; } }
const outer = new Outer();
console.log("nested", outer.o, outer.inner);

let log = [];
class D10 extends Base { c = (log.push("c"), 1); d = (log.push("d"), 2); constructor() { log.push("pre"); super(); log.push("post"); } }
new D10();
console.log("field-order", log.join(","));

class D11 extends Base { ["comp" + "uted"] = 4; constructor() { super(); } }
console.log("computed", new D11().computed);
