// Receiver rows must read through the object's current slots pointer. Objects spill those
// slots after enough properties, while the receiver row continues to describe their shape.
function Holder() {
  this.a0 = 0;
  this.a1 = 1;
  this.a2 = 2;
  this.a3 = 3;
  this.a4 = 4;
  this.a5 = 5;
  this.a6 = 6;
  this.a7 = 7;
  this.a8 = 8;
  this.a9 = 9;
  this.value = null;
  this.anchor = { marker: 42 };
}

Holder.prototype.ensure = function ensure() {
  if (this.value === null) this.value = [this.anchor];
};
Object.defineProperty(Holder.prototype, "lazy", {
  get() {
    this.ensure();
    return this.value;
  },
});

const holder = new Holder();
console.log(
  "spilled-lazy-row",
  holder.lazy[0] === holder.anchor,
  holder.lazy[0] === holder.anchor,
);

// A row learned before a call cannot survive deletion: the object is materialized into a
// dictionary and the old slot remains allocated, but the property no longer exists.
function deleteThenRead() {
  delete this.value;
  return this.value;
}
const deletable = new Holder();
deletable.value = 17;
console.log("deleted-row", deleteThenRead.call(deletable) === undefined);

// An unrelated accessor can mutate `this` without changing its shape. Do not reuse a previous
// direct-this read across that property access.
const receiver = { value: 1 };
const sideEffect = Object.defineProperty({}, "mutate", {
  get() {
    receiver.value = 2;
    return 0;
  },
});
function readAcrossGetter() {
  const before = this.value;
  sideEffect.mutate;
  return [before, this.value];
}
const observed = readAcrossGetter.call(receiver);
console.log("getter-invalidates-copy", observed[0], observed[1]);
