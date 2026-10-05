const proto = {
  get named() { return this.next ? 1 + this.next.named : 0; },
  get computed() { return this.next ? 1 + this.next["computed"] : 0; }
};
let root = Object.create(proto);
for (let i = 0; i < 256; i++) {
  const item = Object.create(proto);
  item.next = root;
  root = item;
}
console.log(root.named, root["computed"]);
let conversions = 0;
let reads = 0;
const key = { [Symbol.toPrimitive]() { conversions++; return "answer"; } };
const target = { marker: 7, get answer() { reads++; return this.marker; } };
console.log(target[key], conversions, reads);
const failure = {};
Object.defineProperty(target, "throws", { get() { throw failure; } });
try { target.throws; } catch (error) { console.log(error === failure); }
class Parent { get value() { return this.marker; } }
class Child extends Parent {
  constructor() { super(); this.marker = 9; }
  get value() { return super.value + super["value"]; }
}
console.log(new Child().value);
