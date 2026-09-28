// CopyDataProperties snapshots own keys before calling a getter, then checks each key's current
// descriptor and value. Deleting an earlier key shifts a dictionary but cannot replace a later
// snapshot key with a newly appended key.
const shifted = { first: 1, get middle() {
  this.added = 4;
  delete this.first;
  return 2;
}, last: 3 };
console.log(JSON.stringify({ ...shifted }));
console.log(JSON.stringify(Object.keys(shifted)));

const changed = { get first() {
  delete this.removed;
  this.last = 9;
  return 1;
}, removed: 2, last: 3 };
console.log(JSON.stringify({ ...changed }));

const hidden = { get first() {
  Object.defineProperty(this, 'later', { enumerable: false });
  return 1;
}, later: 2 };
console.log(JSON.stringify({ ...hidden }));
