// `Reflect.set(target, key, value, receiver)` -- the *fourth* argument, which the native backend
// drops on the floor.
//
// Found while bisecting `vue-reactivity`. The half that stopped that bundle is fixed: the write
// probe read an array's writability out of a property table an array's elements do not live in,
// so `Reflect.set(a, "3", 4, a)` answered false and dropped the element. This is the half that is
// still open, and it is a silent wrong answer rather than a refusal.
//
// Per OrdinarySetWithOwnDescriptor the write lands on the *receiver*, not on the target: the
// target is only where the property is looked up to decide whether the write is allowed at all.
// `kiln_c99_reflect_set` in `crates/kiln-backend-c/src/lib.rs` takes no receiver parameter, and
// the `Reflect.set` dispatch arm never passes `args[3]`, so every write lands on the target.
//
// Vue does not notice, which is why the bundle is not held up by this: its receiver is always a
// proxy over the target whose handler defines neither `getOwnPropertyDescriptor` nor
// `defineProperty`, so the specified path forwards through both traps and ends on the target
// anyway. A receiver that is an ordinary object -- the `Object.create(proto)` and
// class-field-shadowing shapes -- is where the two answers separate.
//
// node and the interpreter agree:
//   plain receiver      true target=0 receiver=5
//   array receiver      true target=1,2,3 receiver=5,8,9
// native:
//   plain receiver      true target=5 receiver=undefined
//   array receiver      true target=5,2,3 receiver=7,8,9
//
// Fixing it means threading a receiver through `kiln_c99_reflect_set` and giving the ordinary
// path the receiver-side half of `OrdinarySetWithOwnDescriptor`, including the partial-descriptor
// redefinition that `reflect-set-receiver.js` in this directory describes -- the interpreter's
// `ordinary_set_reflect` and `define_on_receiver` are the shape to follow.
{
  const target = { x: 0 };
  const receiver = {};
  const accepted = Reflect.set(target, "x", 5, receiver);
  console.log("plain receiver", accepted, "target=" + target.x, "receiver=" + receiver.x);
}
{
  const target = [1, 2, 3];
  const receiver = [7, 8, 9];
  const accepted = Reflect.set(target, "0", 5, receiver);
  console.log("array receiver", accepted, "target=" + target.join(","),
              "receiver=" + receiver.join(","));
}

// Two more disagreements found in the same bisect, both with the *interpreter* on the wrong side.
// They are recorded here rather than in `conformance/native/` because that corpus uses the
// interpreter as its oracle and would read the native backend's correct answer as the defect.
//
//   node and native: Map          interpreter: Object
//   node and native: 3,length     interpreter: 0,1,2,3,length
//   node and native: false        interpreter: true
console.log("map constructor", new Map().constructor === Map, new Map().constructor.name);
{
  const raw = [1, 2, 3];
  const writes = [];
  const proxy = new Proxy(raw, {
    set(t, k, v, r) { writes.push(String(k)); return Reflect.set(t, k, v, r); },
  });
  // `Array.prototype.push` reads `length` and writes one element. The interpreter rewrites every
  // element it walked past, which a set trap can see and vue's would have tracked.
  Array.prototype.push.apply(proxy, [4]);
  console.log("push trap order", writes.join(","));
}
console.log("clear in weakmap", "clear" in new WeakMap());
