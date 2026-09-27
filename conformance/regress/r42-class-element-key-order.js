// Issue 28, item 1: ClassDefinitionEvaluation evaluates every element's key in source order --
// methods, instance fields and static fields alike -- and only then runs the static fields'
// initialisers and the static blocks, in their own source order. Both engines evaluated the
// static fields' keys together with their values, before any method key, and ran them (and the
// static blocks) before the methods were defined. Recorded from node 26.8.1.
const log = [];
const k = (name) => { log.push("key " + name); return name; };
const v = (name) => { log.push("value " + name); return name; };
class A {
  [k("m1")]() {}
  static [k("s1")] = v("s1");
  [k("i1")] = v("i1");
  static [k("m2")]() {}
  static { log.push("block"); }
  static [k("s2")] = v("s2");
  get [k("g1")]() { return 1; }
}
console.log(log.join(", "));
log.length = 0;
new A();
console.log(log.join(", "));

// A static field initialiser sees every method, even one written after it.
class B {
  static early = B.late();
  static late() { return "late"; }
}
console.log(B.early);

// A key that throws stops the class there: no later key runs.
log.length = 0;
try {
  class C {
    [k("a")]() {}
    static [k("b")] = v("b");
    [(() => { throw new Error("stop"); })()]() {}
    [k("c")] = v("c");
  }
} catch (e) {
  console.log(e.message, log.join(", "));
}

// A key's conversion runs where it is evaluated, in the same order.
log.length = 0;
const key = (name) => ({ toString() { log.push("toString " + name); return name; } });
class D {
  static [key("x")] = 1;
  [key("y")]() {}
  [key("z")] = 2;
}
console.log(log.join(", "), Object.keys(D), Object.keys(new D()));

// The heritage is checked before any element key is evaluated.
log.length = 0;
try {
  const n = 1;
  class E extends n { [k("never")] = 1; }
} catch (e) {
  console.log(e.constructor.name, log.length);
}

// A computed static field named `prototype` is refused when it is defined, after every key.
log.length = 0;
try {
  class F { static ["proto" + "type"] = v("p"); [k("after")]() {} }
} catch (e) {
  console.log(e.constructor.name, log.join(", "));
}
