// Bound callables inherit through their mutable [[Prototype]]. This is used by
// Callable-derived classes that return a bound implementation function.
function implementation() {}
const bound = implementation.bind(null);
if (bound.constructor !== Function) throw new Error("default bound constructor");

function Node(attachments, scope) {
  this.attachments = attachments;
  this.scope = scope;
}
Object.defineProperty(Node.prototype, "constructor", {
  value: Node,
  writable: true,
  configurable: true
});
Object.setPrototypeOf(bound, Node.prototype);
if (bound.constructor !== Node) throw new Error("custom prototype constructor");
const cloned = new bound.constructor({ impl: 1 }, "scope");
if (cloned.attachments.impl !== 1 || cloned.scope !== "scope") {
  throw new Error("constructor clone arguments");
}

const own = function Own() {};
Object.defineProperty(bound, "constructor", { value: own, configurable: true });
if (bound.constructor !== own) throw new Error("own constructor takes precedence");
delete bound.constructor;

Object.setPrototypeOf(bound, null);
if (bound.constructor !== undefined) throw new Error("null prototype constructor");
console.log("bound callable constructor lookup ok");
