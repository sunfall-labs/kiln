// `length` is derived, not stored: writing it truncates or extends, which is how a program
// empties an array in place.
const a = [1, 2, 3];
a.length = 1;
console.log(JSON.stringify(a), a.length);
a.length = 3;
console.log(JSON.stringify(a), a.length, 1 in a, a[1]);
const b = [1, 2];
b.length = 0;
console.log(JSON.stringify(b), b.length, b.pop());

// Growing leaves holes, which are not the same as undefined elements.
const holes = [1];
holes.length = 3;
console.log(holes.length, JSON.stringify(holes), holes.filter(() => true).length);

// An impossible length is a RangeError rather than a write that quietly does nothing.
try { [1].length = -1; } catch (error) { console.log(error.constructor.name); }
try { [1].length = 1.5; } catch (error) { console.log(error.constructor.name); }

// A named property of an array is still an ordinary property.
const named = [1];
named.tag = "x";
console.log(named.tag, named.length, JSON.stringify(named));
