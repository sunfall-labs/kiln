// Reflection and descriptor-list walks use the original key order across getter mutations.
const entriesSource = { first: 1, get middle() {
  this.added = 4;
  delete this.first;
  return 2;
}, last: 3 };
console.log(JSON.stringify(Object.keys(entriesSource)));
console.log(JSON.stringify(Object.entries(entriesSource)));
console.log(JSON.stringify(Object.keys(entriesSource)));

const valuesSource = { first: 1, get middle() {
  this.added = 4;
  delete this.first;
  return 2;
}, last: 3 };
console.log(JSON.stringify(Object.values(valuesSource)));

const definitions = { first: { value: 1, enumerable: true }, get middle() {
  this.added = { value: 4, enumerable: true };
  delete this.first;
  return { value: 2, enumerable: true };
}, last: { value: 3, enumerable: true } };
const defined = {};
Object.defineProperties(defined, definitions);
console.log(JSON.stringify(defined));

const ancestor = { inherited: 1, blocked: 2 };
const child = Object.create(ancestor);
Object.defineProperty(child, 'blocked', { value: 3, enumerable: false });
child.own = 4;
const enumerated = [];
for (const key in child) enumerated.push(key);
console.log(JSON.stringify(enumerated));
