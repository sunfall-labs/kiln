// Public class fields define own data properties. They do not assign through inherited setters
// or stop at an inherited non-writable data property.
const parent = Object.defineProperty({}, "field", {
  value: "parent", writable: false, enumerable: true, configurable: true,
});
class FieldTarget extends Object {
  field = "instance";
  ["__proto__"] = "computed proto name";
  static ["__proto__"] = "static computed proto name";
}
Object.setPrototypeOf(FieldTarget.prototype, parent);
const instance = new FieldTarget();
console.log(instance.field, Object.hasOwn(instance, "field"));
console.log(instance["__proto__"], Object.hasOwn(instance, "__proto__"));
console.log(FieldTarget["__proto__"], Object.hasOwn(FieldTarget, "__proto__"));

// A non-index typed-array property belongs to the view and participates in ordinary reflection.
const view = new Uint8Array([3, 4]);
view.label = "bytes";
console.log(view.label, Object.hasOwn(view, "label"), Object.keys(view).join(","));
console.log(Object.getOwnPropertyDescriptor(view, "label").value);
console.log(delete view.label, Object.hasOwn(view, "label"));

// The global object record also sees inherited Object.prototype methods as bare names.
console.log(toString === Object.prototype.toString);
