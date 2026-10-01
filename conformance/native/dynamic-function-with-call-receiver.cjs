// Unqualified calls resolved by a with environment retain its WithBaseObject.
const object = {
  label: "live",
  method() {
    return this.label;
  },
  getterReads: 0,
  get fromGetter() {
    this.getterReads += 1;
    return function () {
      return this.label;
    };
  },
};

const callLive = Function("scope", "with (scope) { return method(); }");
if (callLive(object) !== "live") throw new Error("live with call lost its base object");
console.log("live-with-call", callLive(object));

const strictMethod = Function("", "return function () { 'use strict'; return this; };")();
object.method = strictMethod;
const callGrouped = Function("scope", "with (scope) { return (method)(); }");
const callDoubleGrouped = Function("scope", "with (scope) { return ((method))(); }");
const callDetached = Function("scope", "with (scope) { return (0, method)(); }");
if (callGrouped(object) !== object || callDoubleGrouped(object) !== object)
  throw new Error("parenthesized with reference lost its base object");
if (callDetached(object) !== undefined)
  throw new Error("comma expression incorrectly retained the with reference");
console.log("grouped-with-call", callGrouped(object) === object,
            callDoubleGrouped(object) === object, callDetached(object));

const callGetter = Function("scope", "with (scope) { return fromGetter(); }");
if (callGetter(object) !== "live" || object.getterReads !== 1)
  throw new Error("with getter call lost its base object or ran getter incorrectly");
console.log("live-with-getter-call", object.getterReads);

const captureCall = Function(
  "scope",
  "with (scope) { return function () { return method(); }; }"
);
const captured = captureCall(object);
if (captured() !== object) throw new Error("captured with call lost its base object");
console.log("captured-with-call", captured() === object);

const deleting = {
  label: "deleted",
  get method() {
    delete this.method;
    return strictMethod;
  },
};
if (callGrouped(deleting) !== deleting)
  throw new Error("with getter deletion lost the original reference base");
console.log("deleted-with-getter-call", deleting.method === undefined);
