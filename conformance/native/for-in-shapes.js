// Verification of shape preservation during for...in and own_keys enumeration

// 1. Plain shape object with named properties and dense elements
const obj1 = { a: 1, b: 2 };
obj1[1] = 'one';
obj1[0] = 'zero';
obj1.c = 3;

const keys1 = [];
for (const k in obj1) {
    keys1.push(k);
}
console.log('keys1:', JSON.stringify(keys1));

// Verify fast-path access on obj1 still works after for...in
console.log('obj1.a, b, c:', obj1.a, obj1.b, obj1.c);
console.log('obj1[0], [1]:', obj1[0], obj1[1]);

// 2. Prototype inheritance with shapes
function Parent() {
    this.inheritedProp = 'parent';
}
function Child() {
    this.ownProp = 'child';
}
Child.prototype = new Parent();
const child = new Child();

const keys2 = [];
for (const k in child) {
    keys2.push(k);
}
console.log('keys2:', JSON.stringify(keys2));
console.log('child.ownProp, inheritedProp:', child.ownProp, child.inheritedProp);

// 3. Shadowing across prototype chain
function Base() {
    this.x = 10;
    this.y = 20;
}
function Derived() {
    this.x = 100;
}
Derived.prototype = new Base();
const derived = new Derived();

const keys3 = [];
for (const k in derived) {
    keys3.push(k);
}
console.log('keys3:', JSON.stringify(keys3));
console.log('derived.x, y:', derived.x, derived.y);
