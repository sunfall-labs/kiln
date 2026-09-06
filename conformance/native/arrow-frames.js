// An arrow only needs the enclosing call's frame (`arguments`, `new.target`) live when its own
// code actually reads one of those two things; an arrow that reads only `this` or its own rest
// parameter does not, and is eligible for the direct call path the same as a plain function.
// This corpus exercises the boundary in both directions: arrows that must still see the frame,
// and arrows that must still be *correct* once they no longer need it.

// 1. An arrow reading the enclosing function's `arguments`, called directly and as a map callback.
function argsDirect() {
  const grab = () => [arguments.length, arguments[0]].join(",");
  return grab();
}
console.log("args-direct", argsDirect(10, 20, 30));

function argsViaMap() {
  const grab = () => [arguments.length, arguments[0]].join(",");
  return [0].map(grab)[0];
}
console.log("args-via-map", argsViaMap(11, 22));

// 2. A nested arrow two levels deep reading `arguments` of the outer non-arrow function.
function nestedArgs() {
  const outer = () => () => [arguments.length, arguments[0], arguments[1]].join(",");
  return outer()();
}
console.log("nested-args", nestedArgs(7, 8, 9));

// 3. An arrow inside a method whose code never reads `new.target` (or `arguments`): eligible for
// the fast direct path. Paired with a constructor whose arrow *does* read `new.target`, which
// must keep seeing the constructor's own -- including when a subclass with no constructor of its
// own reaches it through `super()`.
class Widget {
  constructor(label) {
    this.label = label;
    this.describe = () => `widget:${this.label}`;
  }
}
const widget = new Widget("gadget");
console.log("method-arrow", widget.describe(), [0].map(widget.describe)[0]);

function Ctor(...rest) {
  this.rest = rest;
  this.captured = () => (new.target ? new.target.name : "none");
}
class SubCtor extends Ctor {}
console.log("new-target-plain", new Ctor(1, 2).captured());
console.log("new-target-sub", new SubCtor(3, 4).captured());

// 4. An arrow using rest params: `RestArguments` reads the body's own `args`/`argc`, not the
// frame globals, so this alone must not force the frame.
const restLen = (...xs) => xs.length;
console.log("rest-len", restLen(1, 2, 3, 4), restLen());

function usesRestArrow() {
  const inner = (...xs) => xs.reduce((a, b) => a + b, 0);
  return inner(1, 2, 3);
}
console.log("rest-sum", usesRestArrow());

// 5. Arrows capturing `this` in a class method, used as map/filter/reduce callbacks and handed to
// another function that calls them with a different receiver -- which must not matter: the
// direct-call path's receiver for an arrow is its own captured `this`, never the caller's.
class Scaler {
  constructor(factor) {
    this.factor = factor;
  }
  scaleAll(list) {
    return list.map((x) => x * this.factor);
  }
  keepAboveAll(list) {
    return list.filter((x) => x * this.factor > this.factor);
  }
  sumScaledAll(list) {
    return list.reduce((acc, x) => acc + x * this.factor, 0);
  }
  makeScaler() {
    return (x) => x * this.factor;
  }
}
const scaler = new Scaler(3);
console.log("scale-map", scaler.scaleAll([1, 2, 3]).join(","));
console.log("scale-filter", scaler.keepAboveAll([0, 1, 2]).join(","));
console.log("scale-reduce", scaler.sumScaledAll([1, 2, 3]));

const capturedFn = scaler.makeScaler();
const otherReceiver = { factor: 1000 };
function callWithOtherReceiver(fn, receiver) {
  return fn.call(receiver, 5);
}
console.log("captured-this-direct", capturedFn(5));
console.log("captured-this-other-receiver", callWithOtherReceiver(capturedFn, otherReceiver));
console.log("captured-this-map", [1, 2].map(capturedFn).join(","));

// Bonus: an arrow that recurses through its own binding (the "self-call" direct path) while
// still reading a captured `this` -- the direct path must keep re-passing that same captured
// value down the recursion rather than trying to read it off a callee that self-recursion never
// loads.
const counter = { seed: 100, countDown: null };
counter.countDown = function (n) {
  const step = (k) => (k <= 0 ? this.seed : step(k - 1));
  return step(n);
};
console.log("self-recursive-this", counter.countDown(5));

// 6. The array-functional pattern itself: map/filter/reduce with arithmetic arrows, and a
// checksum over a larger input so a wrong fast path shows up as a wrong number, not a crash.
function arrayFunctionalChecksum(n) {
  const data = [];
  for (let i = 0; i < n; i++) data.push(i);
  return data
    .map((x) => x * 2 + 1)
    .filter((x) => x % 3 !== 0)
    .reduce((acc, x) => acc + x, 0);
}
console.log("array-functional", arrayFunctionalChecksum(1000));
