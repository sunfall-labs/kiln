// Polymorphic inline cache verification: loads, stores, and transitions across 2-4 shapes.

function getX(point) {
    return point.x;
}

function setX(point, val) {
    point.x = val;
}

// 1. Monomorphic -> Polymorphic loads across 4 different shapes
const p1 = { x: 1 };
const p2 = { a: 10, x: 2 };
const p3 = { a: 10, b: 20, x: 3 };
const p4 = { a: 10, b: 20, c: 30, x: 4 };

// Prime PIC
console.log('load p1:', getX(p1));
console.log('load p2:', getX(p2));
console.log('load p3:', getX(p3));
console.log('load p4:', getX(p4));

// Loop to verify repeated PIC hits
let sum = 0;
for (let i = 0; i < 100; i++) {
    sum += getX(p1) + getX(p2) + getX(p3) + getX(p4);
}
console.log('sum 100 iters:', sum);

// 2. Polymorphic stores
setX(p1, 100);
setX(p2, 200);
setX(p3, 300);
setX(p4, 400);
console.log('after setX:', p1.x, p2.x, p3.x, p4.x);

// 3. Megamorphic (> 4 shapes) fallback
const p5 = { a: 1, b: 2, c: 3, d: 4, x: 5 };
const p6 = { a: 1, b: 2, c: 3, d: 4, e: 5, x: 6 };
console.log('megamorphic load p5:', getX(p5));
console.log('megamorphic load p6:', getX(p6));
setX(p5, 500);
setX(p6, 600);
console.log('megamorphic store p5, p6:', p5.x, p6.x);

// 4. Polymorphic transitioning store
function addY(obj, val) {
    obj.y = val;
}

const t1 = { u: 1 };
const t2 = { v: 2, w: 3 };
const t3 = { p: 4, q: 5, r: 6 };
addY(t1, 11);
addY(t2, 22);
addY(t3, 33);
console.log('t1:', JSON.stringify(t1));
console.log('t2:', JSON.stringify(t2));
console.log('t3:', JSON.stringify(t3));

// Verify repeated access to transitioned properties
console.log('transitioned y sum:', t1.y + t2.y + t3.y);
