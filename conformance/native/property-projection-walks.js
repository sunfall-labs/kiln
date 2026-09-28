// The shared descriptor/value projection must keep key order and copy each property record
// before a getter can mutate the source's dictionary or trigger collection.
const dense = { 2: 'two', 0: 'zero', alpha: 1, beta: 2 };
console.log(JSON.stringify(Object.keys(dense)));
console.log(JSON.stringify({ ...dense }));

const source = { first: 1, get middle() {
  for (let i = 0; i < 40; i++) this['added' + i] = i;
  return 2;
}, last: 3 };
console.log(JSON.stringify(Object.keys(source)));
console.log(JSON.stringify({ ...source }));
console.log(Object.keys(source).length, source.added39);

const jsonSource = { first: 1, get middle() {
  this.added = 4;
  return 2;
}, last: 3 };
console.log(JSON.stringify(jsonSource));
console.log(JSON.stringify(Object.keys(jsonSource)));

const array = [7, , 9];
array.extra = 5;
console.log(JSON.stringify(Object.keys(array)));
