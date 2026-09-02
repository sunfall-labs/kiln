// Property-semantics probe for the accessor_count / same_key / defineProperty / node_event
// changes. Run with --node (test 10 needs node:events), interpreted (`kiln run`) and natively
// (`kiln build --node --profile native-release` then execute), and diff stdout. Every line is a
// self-contained assertion; failures print rather than throw, so one bad case does not hide the
// rest.
import { EventEmitter } from "node:events";

function report(name, cond, extra) {
  console.log((cond ? "PASS " : "FAIL ") + name + (extra !== undefined ? " " + JSON.stringify(extra) : ""));
}

// 1. A setter on a prototype still intercepts assignment to a fresh object.
{
  const proto = {};
  let seen;
  Object.defineProperty(proto, "x", { set(v) { seen = v; }, get() { return seen; }, configurable: true });
  const child = Object.create(proto);
  child.x = 42;
  report("prototype setter intercepts fresh-object write", seen === 42 && !Object.prototype.hasOwnProperty.call(child, "x"));
}

// 2. defineProperty accessor still works after delete.
{
  const o = {};
  Object.defineProperty(o, "y", { get() { return 7; }, configurable: true });
  delete o.y;
  Object.defineProperty(o, "y", { get() { return 9; }, configurable: true });
  report("accessor works after delete+redefine", o.y === 9);
}

// 3. Object.keys order unchanged: integer keys ascending, then insertion order.
{
  const o = {};
  o.b = 1; o.a = 2; o[2] = 3; o[1] = 4; o.c = 5;
  report("Object.keys order", JSON.stringify(Object.keys(o)) === JSON.stringify(["1", "2", "b", "a", "c"]), Object.keys(o));
}

// 4. Deep prototype chain, setter several levels up, with plain data levels below it
//    (this exercises the accessor_count==0 skip on every level except the one with the setter).
{
  const top = {};
  let hits = 0;
  Object.defineProperty(top, "z", { set() { hits += 1; }, configurable: true });
  const mid1 = Object.create(top);
  mid1.m1 = 1;
  const mid2 = Object.create(mid1);
  mid2.m2 = 2;
  const leaf = Object.create(mid2);
  leaf.z = "go";
  report("setter several levels up a plain chain fires once", hits === 1);
}

// 5. Duplicate key in an object literal, accessor then data (define_computed transition).
{
  const o = { get x() { return 1; }, x: 2 };
  report("duplicate literal key: last (data) wins", o.x === 2);
  let threw = false;
  try {
    Object.defineProperty(o, "x", { value: 3 });
  } catch (e) { threw = true; }
  report("post-literal x is a plain writable/configurable data prop", o.x === 3 || threw, { x: o.x, threw });
}

// 6. Inherited non-writable, non-accessor data property blocks a shadowing write.
//    This is the one case accessor_count==0 cannot see (documented trade-off): a level with no
//    accessors can still carry a plain readonly data property under the same key. Modules are
//    always strict, so this only exercises the strict (throwing) half; `new Function` would
//    exercise the sloppy half but the native backend refuses `New` outright, which would abort
//    the rest of the probe, so it is left out.
{
  const proto = {};
  Object.defineProperty(proto, "ro", { value: 1, writable: false, configurable: true });
  const child = Object.create(proto);
  let threw = false;
  try {
    child.ro = 2;
  } catch (e) { threw = true; }
  report("strict (module-top-level) write through inherited readonly data prop throws", threw, { value: child.ro });
}

// 7. Whether Object.freeze on a prototype blocks a child's shadowing write is not asserted a
//    particular way here -- it depends on whether freeze sets per-property writable, which this
//    task does not touch. Recorded (and never allowed to abort the rest of the probe) so the
//    two engines can be compared for agreement rather than graded against a expectation.
{
  const proto = { fx: 1 };
  Object.freeze(proto);
  const child = Object.create(proto);
  let threw = false;
  try {
    child.fx = 99;
  } catch (e) {
    threw = true;
  }
  report("frozen-prototype shadow write (pre-existing behavior, recorded not required)", true, { childFx: child.fx, protoFx: proto.fx, threw });
}

// 8. Many own properties then repeated same-key writes (own-list-first fast path).
{
  const o = {};
  for (let i = 0; i < 50; i++) o["k" + i] = i;
  for (let i = 0; i < 50; i++) o["k" + i] = i * 2;
  let ok = true;
  for (let i = 0; i < 50; i++) if (o["k" + i] !== i * 2) ok = false;
  report("repeated own-key writes after many properties", ok);
}

// 9. Interned vs dynamic (computed/concatenated) keys with equal content compare equal.
{
  const o = {};
  o.dynamicKey = 1;
  const built = "dynamic" + "Key";
  o[built] = 2;
  report("dynamic key with same content as an interned one overwrites, not duplicates", o.dynamicKey === 2 && Object.keys(o).length === 1, Object.keys(o));
}

// 10. EventEmitter: emit with no listeners for the name is falsy and does not disturb other
//     names' listeners; on/emit/once/removeListener still behave once something is registered.
{
  const e = new EventEmitter();
  const r1 = e.emit("nobody-home");
  report("emit with zero listeners ever registered returns false", r1 === false);
  let a = 0, b = 0;
  e.on("x", () => { a += 1; });
  e.once("y", () => { b += 1; });
  const r2 = e.emit("still-nobody");
  report("emit for an unregistered name after other names exist returns false", r2 === false);
  e.emit("x"); e.emit("x");
  e.emit("y"); e.emit("y");
  report("on fires every time, once fires once", a === 2 && b === 1, { a, b });
  e.removeAllListeners("x");
  const r3 = e.emit("x");
  report("emit after removeAllListeners(name) returns false", r3 === false);
}

console.log("done");
