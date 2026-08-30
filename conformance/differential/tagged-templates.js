// A tagged template is a call whose first argument is the template object.
function tag(strings, ...values) {
  return strings.length + "|" + strings.join("~") + "|" + values.join(",");
}
console.log(tag`a${1}b${2}c`);
console.log(tag`no substitutions`);
console.log(tag`${"only"}`);

// The template object carries the raw text alongside the cooked text.
function both(strings) {
  return strings[0] + " / " + strings.raw[0];
}
console.log(both`a\nb`);
console.log(String.raw`c:\dir\file`);
console.log(String.raw`tab\there${1}and\\more`);

// The tag is evaluated as a callee, so a method form keeps its receiver.
const holder = {
  name: "holder",
  run(strings) {
    return this.name + ":" + strings[0];
  },
};
console.log(holder.run`x`);

// Substitutions are ordinary expressions, evaluated left to right.
let order = "";
const note = (value) => {
  order += value;
  return value;
};
tag`${note("1")}${note("2")}${note("3")}`;
console.log(order);

// A tag can ignore its strings entirely, which is how most DSL tags start.
const first = (strings, ...values) => values[0];
console.log(first`ignored ${"kept"} ignored`);

// Both halves of the template object are frozen: a tag that changed the object it was handed
// would be changing something its caller may hold.
function shape(strings) {
  return [Object.isFrozen(strings), Object.isFrozen(strings.raw)];
}
console.log(shape`a${1}b`.join(","));

// Two *different* call sites produce different objects, which is what the specification says.
function twice() {
  return [((s) => s)`x`, ((s) => s)`x`];
}
const [siteA, siteB] = twice();
console.log(siteA === siteB, siteA[0] === siteB[0]);
