// `Buffer.concat` copies bytes in bulk, and a bulk copy is where offsets go wrong.
//
// It used to copy element by element through the generic typed-array accessors: each byte
// dispatched on the view's kind, bounds-checked, widened to a double and narrowed back. Both
// sides of a `Buffer.concat` are always `Uint8Array`, so that is a `memcpy` written as a loop --
// 76 MiB of it took 207ms where node took 2.9ms, about 370 MiB/s against memcpy's tens of
// gigabytes.
//
// The fast path is only correct if it honours what the slow path did implicitly. A `subarray`
// shares its storage and starts partway into it, so the copy has to read from `byteOffset` rather
// than from the front of the buffer; an empty entry must not advance the write cursor; and a byte
// above 0x7f must survive a round trip that no longer passes through a double. Those are the
// three this probe exists for, and none of them is visible in a concat of two whole buffers.
const a = Buffer.from("hello");
const b = Buffer.from(" world");
const empty = Buffer.alloc(0);
console.log(Buffer.concat([a, b]).toString("utf8"));
console.log(Buffer.concat([empty, a, empty, b, empty]).toString("utf8"));
console.log(Buffer.concat([]).length, Buffer.concat([a]).toString("utf8"));

const big = Buffer.from("0123456789");
const mid = big.subarray(3, 7);
console.log(mid.toString("utf8"), mid.length);
console.log(Buffer.concat([mid, mid]).toString("utf8"));
console.log(Buffer.concat([big.subarray(8), big.subarray(0, 2)]).toString("utf8"));

const high = Buffer.from([0, 1, 127, 128, 200, 255]);
console.log(Array.from(Buffer.concat([high, high])).join(","));
console.log(Buffer.concat([high], 3).toString("hex"));
console.log(Buffer.concat([high.subarray(2, 4)], 5).toString("hex"));

const typed = new Uint8Array([9, 8, 7, 6]);
console.log(Buffer.concat([typed.subarray(1, 3), new Uint8Array([5])]).toString("hex"));
console.log(Buffer.concat([typed], 2).toString("hex"));
console.log(Buffer.concat([typed.subarray(1, 3)], 5).toString("hex"));
console.log("__kilnBufferTypedArrayCopy" in globalThis);
