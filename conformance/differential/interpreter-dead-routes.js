// These public built-ins were implemented but had no VM dispatch route. Keeping the calls in one
// source probe makes a missing route fail as a diagnostic or process abort instead of hiding behind
// an otherwise clean native marker table.
const objectGroups = Object.groupBy([1.2, 2.4, 2.9], Math.floor);
console.log(JSON.stringify(objectGroups));

const mapGroups = Map.groupBy([1.2, 2.4, 2.9], Math.floor);
console.log([...mapGroups].map(([key, values]) => key + ":" + values.join(",")).join("|"));

const object = {};
object.__defineGetter__("answer", () => 42);
console.log(object.__lookupGetter__("answer")());

const iterator = Iterator.from([1, 2]).map((value) => value * 2);
console.log(iterator.next().value, iterator.next().value, iterator.next().done);

const regexp = /a/g;
console.log(regexp[Symbol.match]("cat").join("|"), regexp[Symbol.replace]("cat", "x"));
