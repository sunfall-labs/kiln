// Conformance test for dense element fast-paths and computed access on arrays and shape objects

// 1. Array indexed reads and writes
const arr = [10, 20, 30, 40, 50];
for (let i = 0; i < arr.length; i++) {
    arr[i] = arr[i] * 2;
}
for (let i = 0; i < arr.length; i++) {
    console.log("arr", i, arr[i]);
}

// 2. Shape object with dense elements
const obj = { name: "test_obj" };
for (let i = 0; i < 5; i++) {
    obj[i] = (i + 1) * 100;
}
for (let i = 0; i < 5; i++) {
    console.log("obj elem", i, obj[i]);
}
console.log("obj name:", obj.name);

// 3. Fused computed reads and writes in loop
const src = [1, 2, 3, 4, 5];
const dst = [0, 0, 0, 0, 0];
for (let i = 0; i < 5; i++) {
    const val = src[i];
    dst[i] = val + 10;
}
for (let i = 0; i < 5; i++) {
    console.log("dst", i, dst[i]);
}

// 4. Object literal with dense elements and length
const listLike = { length: 3, 0: "alpha", 1: "beta", 2: "gamma" };
for (let i = 0; i < listLike.length; i++) {
    console.log("listLike", i, listLike[i]);
}
console.log("listLike keys:", JSON.stringify(Object.keys(listLike)));

// 5. Out-of-bounds reads
console.log("arr oob:", arr[100]);
console.log("obj oob:", obj[100]);
