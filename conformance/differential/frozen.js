// Freezing an array has to reach its elements, which are not properties of its property store.
const list = Object.freeze([1, 2]);
list[0] = 9;
console.log(JSON.stringify(list), Object.isFrozen(list), list.length);
try { list.push(3); } catch (error) { console.log("push", error.constructor.name); }
try { list.pop(); } catch (error) { console.log("pop", error.constructor.name); }
console.log(JSON.stringify(list));

// The methods that do not change the length still work on a frozen array.
console.log(list.join("-"), list.indexOf(2), list.slice(0, 1).join(""));

// An object behaves the same way, and a frozen function's properties too.
const object = Object.freeze({ x: 1 });
object.x = 2;
object.y = 3;
console.log(JSON.stringify(object), Object.isFrozen(object));
function fn() {}
Object.freeze(fn);
fn.tag = "x";
console.log(fn.tag, Object.isFrozen(fn));

// Nothing else is frozen unless it was frozen.
console.log(Object.isFrozen([1]), Object.isFrozen({}), Object.isFrozen(1), Object.isFrozen("s"));
