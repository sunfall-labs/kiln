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

const callGetter = Function("scope", "with (scope) { return fromGetter(); }");
if (callGetter(object) !== "live" || object.getterReads !== 1)
  throw new Error("with getter call lost its base object or ran getter incorrectly");
console.log("live-with-getter-call", object.getterReads);

const captureCall = Function(
  "scope",
  "with (scope) { return function () { return method(); }; }"
);
const captured = captureCall(object);
object.method = function () {
  return this.label + "-updated";
};
if (captured() !== "live-updated") throw new Error("captured with call lost its base object");
console.log("captured-with-call", captured());
