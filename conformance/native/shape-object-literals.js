// Conformance test for shape-preserving object literals, pre-sized slots, and inline caches

function makePoint(x, y) {
    return { x: x, y: y };
}

function make3D(x, y, z) {
    return { x: x, y: y, z: z };
}

function getSum(p) {
    return p.x + p.y;
}

// Test monomorphic IC on object literals
const points = [];
for (let i = 0; i < 5; i++) {
    points.push(makePoint(i, i * 10));
}

for (let i = 0; i < points.length; i++) {
    console.log("sum", i, getSum(points[i]));
}

// Test polymorphic IC between 2D and 3D object literals
const mixed = [
    makePoint(1, 2),
    make3D(3, 4, 5),
    makePoint(6, 7),
    make3D(8, 9, 10)
];

for (let i = 0; i < mixed.length; i++) {
    console.log("mixed", i, "x:", mixed[i].x, "y:", mixed[i].y, "sum:", getSum(mixed[i]));
}

// Object literal with duplicate keys
const dup = { a: 1, b: 2, a: 3 };
console.log("dup:", dup.a, dup.b, JSON.stringify(Object.keys(dup)));

// Object literal with dense elements
const arrayLike = { length: 3, 0: "first", 1: "second", 2: "third" };
console.log("arrayLike Array.from:", JSON.stringify(Array.from(arrayLike)));
console.log("arrayLike keys:", JSON.stringify(Object.keys(arrayLike)));

// Object literal with 6 properties (exceeding inline 4 slots)
const large = { a: 1, b: 2, c: 3, d: 4, e: 5, f: 6 };
console.log("large:", large.a, large.b, large.c, large.d, large.e, large.f);
console.log("large JSON:", JSON.stringify(large));

// Object.defineProperties with object literal descriptor map
const target = {};
Object.defineProperties(target, {
    prop1: { value: 42, enumerable: true },
    prop2: { value: "hello", enumerable: false }
});
console.log("defineProperties:", target.prop1, target.prop2, JSON.stringify(Object.keys(target)));
