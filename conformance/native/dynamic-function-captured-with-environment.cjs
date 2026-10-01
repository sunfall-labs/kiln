// A function created while a `with` record is active keeps that same object environment after
// the creating call returns, including its relationship to outer lexical bindings.
const makeReader = Function(
  "scope",
  "name",
  "with (scope) { return function() { return name; }; }"
);
const object = { name: "first", value: 1 };
const read = makeReader(object, "outer");
if (read() !== "first") throw new Error("captured with record lost to outer lexical binding");
console.log("with-live", object.name);
object.name = "second";
if (read() !== "second") throw new Error("captured with record lost object identity");
console.log("with-identity", object.name);

const makeWriter = Function(
  "scope",
  "with (scope) { return function() { value = value + 1; return value; }; }"
);
const write = makeWriter(object);
const written = write();
if (written !== 2 || object.value !== 2) throw new Error("captured with assignment missed object");
console.log("with-write", written, object.value);

const hidden = { name: "hidden" };
hidden[Symbol.unscopables] = { name: true };
const readOuter = makeReader(hidden, "outer");
if (readOuter() !== "outer") throw new Error("captured with ignored Symbol.unscopables");
console.log("with-unscopables", readOuter());

const makeNestedReader = Function(
  "scope",
  "with (scope) { return function() { return function() { return name; }; }; }"
);
const nested = makeNestedReader(object, "outer")()();
if (nested() !== "second") throw new Error("nested closure lost with record identity");
console.log("nested-with-live", nested());
