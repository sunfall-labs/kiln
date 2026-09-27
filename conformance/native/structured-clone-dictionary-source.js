// `structuredClone` of objects the native object model keeps in list (dictionary) mode: one that
// lost a property, one past the fast-property limit, one whose keys include an index. Since
// 2026-09-27 the plain-data fast path (`__kilnCloneData`, runtime/spread.c) gives a list-mode
// source's copy the shape an object literal with the same keys has, where it used to copy the
// dictionary; a copy must still have the source's keys, in the source's order, with the
// source's values, and must stay independent of it.
function describe(value) {
  return JSON.stringify(value) + " " + Object.keys(value).length;
}

const deleted = { id: 1, name: "item", extra: true, tags: ["a", "b"], price: 2.5 };
delete deleted.extra;
const deletedCopy = structuredClone(deleted);
deletedCopy.tags.push("c");
console.log(describe(deletedCopy), deleted.tags.length, deletedCopy.price + deletedCopy.id);

const wide = {};
for (let i = 0; i < 80; i += 1) wide["k" + i] = i;
delete wide.k3;
const wideCopy = structuredClone(wide);
console.log(Object.keys(wideCopy).length, wideCopy.k79, wideCopy.k3, Object.keys(wideCopy)[3]);

const indexed = { b: 1, 2: "two", a: 2, 0: "zero" };
delete indexed.b;
console.log(describe(structuredClone(indexed)));

const records = [];
for (let i = 0; i < 50; i += 1) {
  const record = { id: i, gone: 0, label: "r" + i, nested: { v: i * 2 } };
  delete record.gone;
  records.push(record);
}
const copies = structuredClone(records);
let sum = 0;
for (let i = 0; i < copies.length; i += 1) sum += copies[i].id + copies[i].nested.v + copies[i].label.length;
console.log(sum, copies[49].label, Object.keys(copies[7]).join(","), copies[3].nested !== records[3].nested);
