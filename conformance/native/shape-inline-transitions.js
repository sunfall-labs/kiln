// Conformance test for shape inline transitions and polymorphic ICs

function Point2D(x, y) {
    this.x = x;
    this.y = y;
}

function Point3D(x, y, z) {
    this.x = x;
    this.y = y;
    this.z = z;
}

function PointColor(x, y, color) {
    this.x = x;
    this.y = y;
    this.color = color;
}

const pts = [
    new Point2D(10, 20),
    new Point3D(30, 40, 50),
    new PointColor(60, 70, "red"),
    new Point2D(80, 90)
];

// Test polymorphic read ICs across 3 different shapes
function readX(obj) {
    return obj.x;
}
function readY(obj) {
    return obj.y;
}

for (let i = 0; i < pts.length; i++) {
    console.log("pt", i, "x:", readX(pts[i]), "y:", readY(pts[i]));
}

// Test polymorphic write ICs
function setX(obj, val) {
    obj.x = val;
}

for (let i = 0; i < pts.length; i++) {
    setX(pts[i], pts[i].x * 2);
    console.log("pt", i, "doubled x:", pts[i].x);
}

// Test transitioning store in loop (constructing new objects)
for (let j = 0; j < 5; j++) {
    const o = {};
    o.a = j;
    o.b = j * 10;
    o.c = j * 100;
    console.log("o" + j, o.a, o.b, o.c, JSON.stringify(Object.keys(o)));
}
