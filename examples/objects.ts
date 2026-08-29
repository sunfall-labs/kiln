// Objects have identity: two references name the same object, and a write through one is
// visible through the other.
const point = { x: 1, y: 2 };
const alias = point;
alias.x = 10;
console.log("shared", point.x, alias.x);
console.log("identity", point === alias, point === { x: 10, y: 2 });

// Properties can be added after construction, and a missing one reads as undefined rather
// than failing.
point.label = "origin";
console.log("added", point.label, "missing", point.z);

// Insertion order is observable, so re-assigning a key must not move it.
const ordered = { first: 1, second: 2 };
ordered.first = 99;
console.log("reassigned in place", ordered.first, ordered.second);

// A compound assignment evaluates its receiver once.
let calls = 0;
function receiver() {
  calls += 1;
  return point;
}
receiver().x += 5;
console.log("receiver calls", calls, "x", point.x);

function makePoint(x: number, y: number) {
  return { x: x, y: y };
}
const made = makePoint(3, 4);
console.log("constructed", made.x, made.y);
