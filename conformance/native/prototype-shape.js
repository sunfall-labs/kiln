// Prototype-aware shape object model: constructors, inheritance, and prototype setters/read-only

// 1. Constructor instances sharing shapes and transitions
function Point(x, y) {
    this.x = x;
    this.y = y;
}
Point.prototype.dist = function() {
    return this.x * this.x + this.y * this.y;
};

const p1 = new Point(3, 4);
const p2 = new Point(5, 12);

console.log('p1:', p1.x, p1.y, p1.dist());
console.log('p2:', p2.x, p2.y, p2.dist());

// 2. Object.keys/values/entries on constructor instances (must see own shape properties)
console.log('keys p1:', JSON.stringify(Object.keys(p1)));
console.log('keys p2:', JSON.stringify(Object.keys(p2)));
console.log('values p1:', JSON.stringify(Object.values(p1)));

// 3. Monomorphic and polymorphic field access across multiple instances
function getX(pt) {
    return pt.x;
}
function getY(pt) {
    return pt.y;
}
console.log('getX loop:', getX(p1), getX(p2), getX(p1));
console.log('getY loop:', getY(p1), getY(p2), getY(p1));

// 4. Mutation of existing property
p1.x = 9;
console.log('mutated p1.x:', p1.x, p1.dist());

// 5. Prototype setter interception
const protoWithSetter = {
    set custom(v) {
        this._custom = v * 10;
    }
};
const child = Object.create(protoWithSetter);
child.custom = 5;
console.log('child custom intercepted:', child.custom, child._custom);

// 6. Non-writable prototype property blocks own assignment
const protoWithReadOnly = {};
Object.defineProperty(protoWithReadOnly, 'ro', {
    value: 'read-only-val',
    writable: false,
    configurable: true
});
const childRO = Object.create(protoWithReadOnly);
try {
    childRO.ro = 'attempted-write';
} catch (e) {
    console.log('caught read-only write error');
}
console.log('childRO ro preserved:', childRO.ro);

// 7. Adding new property to constructor instance
p1.z = 100;
console.log('p1.z:', p1.z);
console.log('keys p1 after z:', JSON.stringify(Object.keys(p1)));
console.log('keys p2 unchanged:', JSON.stringify(Object.keys(p2)));
