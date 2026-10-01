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
const asyncScope = { async: strictMethod };
const callGroupedAsync = Function("scope", "with (scope) { return (async)(); }");
const callDoubleGroupedAsync = Function("scope", "with (scope) { return ((async))(); }");
const callDetachedAsync = Function("scope", "with (scope) { return (0, async)(); }");
if (callGroupedAsync(asyncScope) !== asyncScope ||
    callDoubleGroupedAsync(asyncScope) !== asyncScope ||
    callDetachedAsync(asyncScope) !== undefined)
  throw new Error("contextual async identifier lost or invented its with reference");
const asyncExpressionFallback = Function("return (async function () { return 1; });")();
if (typeof asyncExpressionFallback !== "function")
  throw new Error("parenthesized async function expression was treated as an identifier");
console.log("grouped-with-call", callGrouped(object) === object,
            callDoubleGrouped(object) === object, callDetached(object));
console.log("grouped-async-with-call", callGroupedAsync(asyncScope) === asyncScope,
            callDoubleGroupedAsync(asyncScope) === asyncScope, callDetachedAsync(asyncScope));

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
