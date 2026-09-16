// `parallelMap` on both engines (ADR 0058, application 2). The native backend runs a callback it
// can prove reads its double parameters and returns a double on the provider's pool; every other
// callback takes the sequential lowering. Either way the answer is the interpreter's, which is
// what this probe compares. Nothing here is refused.

// 1. A qualifying numeric callback over enough elements to clear the fork-join floor.
const big: number[] = [];
for (let i = 0; i < 100000; i++) big.push(i);
const squared = (big as any).parallelMap((x: number) => Math.sqrt(x * x + 1) * 2 - Math.floor(x / 3));
let checksum = 0;
for (let i = 0; i < squared.length; i++) checksum += squared[i];
console.log("qualifying", squared.length, checksum);

// 2. With the index parameter.
const indexed = (big as any).parallelMap((x: number, i: number) => x - i + Math.abs(-i));
console.log("indexed", indexed.length, indexed[0], indexed[99999]);

// 3. A callback that allocates: sequential, and the answer is arrays.
const boxed = ([1, 2, 3] as any).parallelMap((x: number) => [x]);
console.log("allocates", boxed.length, Array.isArray(boxed[0]), boxed[2][0]);

// 4. A callback reading a heap upvalue: sequential.
const table = { scale: 10 };
const scaled = ([1, 2, 3] as any).parallelMap((x: number) => x * table.scale);
console.log("upvalue", scaled.join(","));

// 5. A hole: the receiver is not numeric, so sequential, and the hole stays a hole.
const holey = [1, , 3] as any[];
const kept = holey.parallelMap((x: number) => x * 2);
console.log("hole", kept.length, 1 in kept, kept[0], kept[2]);

// 6. A callback returning a string: sequential, and the answer is strings.
const named = ([1, 2, 3] as any).parallelMap((x: number) => "n" + x);
console.log("string", named.join("|"));

// 7. A small numeric array, below the floor, still through the same lowering.
const small = ([3, 1, 2] as any).parallelMap((x: number) => x * x);
console.log("small", small.join(","));

// 8. Transcendental and trigonometric math builtins in parallelMap.
const bigTrig: number[] = [];
for (let i = 0; i < 50000; i++) bigTrig.push(i);
const trig = (bigTrig as any).parallelMap((x: number) => {
    return Math.sin(x) + Math.cos(x) + Math.hypot(x, 1) + Math.log(x + 1) + Math.exp(-x);
});
let trigSum = 0;
for (let i = 0; i < 100; i++) trigSum += Math.round(trig[i] * 1000);
console.log("transcendental", trig.length, trigSum);

// 9. TypedArray parallelMap across various kinds and pooled execution.
const u8Src = new Uint8Array([10, 20, 30]);
const u8Out = (u8Src as any).parallelMap((x: number) => x * 2);
console.log("typed u8", u8Out instanceof Uint8Array, u8Out[0], u8Out[1], u8Out[2]);

const i8Src = new Int8Array([-10, 20, -30]);
const i8Out = (i8Src as any).parallelMap((x: number) => x * 2);
console.log("typed i8", i8Out instanceof Int8Array, i8Out[0], i8Out[1], i8Out[2]);

const u8cSrc = new Uint8ClampedArray([200, 250, 10]);
const u8cOut = (u8cSrc as any).parallelMap((x: number) => x + 20.5);
console.log("typed u8c", u8cOut instanceof Uint8ClampedArray, u8cOut[0], u8cOut[1], u8cOut[2]);

const i16Src = new Int16Array([-1000, 2000, -3000]);
const i16Out = (i16Src as any).parallelMap((x: number) => x * 2);
console.log("typed i16", i16Out instanceof Int16Array, i16Out[0], i16Out[1], i16Out[2]);

const u16Src = new Uint16Array([1000, 2000, 3000]);
const u16Out = (u16Src as any).parallelMap((x: number) => x * 2);
console.log("typed u16", u16Out instanceof Uint16Array, u16Out[0], u16Out[1], u16Out[2]);

const i32Src = new Int32Array([-1000, 2000, 3000]);
const i32Out = (i32Src as any).parallelMap((x: number, i: number) => x + i);
console.log("typed i32", i32Out instanceof Int32Array, i32Out[0], i32Out[1], i32Out[2]);

const u32Src = new Uint32Array([100000, 200000, 300000]);
const u32Out = (u32Src as any).parallelMap((x: number) => x * 2);
console.log("typed u32", u32Out instanceof Uint32Array, u32Out[0], u32Out[1], u32Out[2]);

const f32Src = new Float32Array([1.5, 2.5, 3.5]);
const f32Out = (f32Src as any).parallelMap((x: number) => x * 2.0);
console.log("typed f32", f32Out instanceof Float32Array, Math.round(f32Out[0]), Math.round(f32Out[1]), Math.round(f32Out[2]));

const f64Src = new Float64Array(10000);
for (let i = 0; i < f64Src.length; i++) f64Src[i] = i * 0.5;
const f64Out = (f64Src as any).parallelMap((x: number) => Math.sqrt(x + 1.0) * 2.0);
console.log("typed f64 pooled", f64Out instanceof Float64Array, f64Out.length, Math.round(f64Out[0] * 100), Math.round(f64Out[9999] * 100));

