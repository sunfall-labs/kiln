// Buffer.prototype (issue 08 of .scratch/full-runtime): the shared TypeScript prototype over each
// engine's branded Uint8Array. Every line is node-identical in both engines as of 2026-09-19.
const b = Buffer.from("héllo wörld", "utf8");
console.log(typeof Buffer.prototype, Object.getPrototypeOf(b) === Buffer.prototype, b instanceof Buffer, b instanceof Uint8Array, Buffer.isBuffer(b), Buffer.isBuffer(new Uint8Array(1)), b.constructor === Buffer);
console.log(b.length, b.toString("hex").slice(0, 8), b.toString("base64"), Buffer.concat([b.subarray(0, 3), Buffer.from("!")]).toString(), b.equals(Buffer.from(b)), Buffer.byteLength("é"), b.readUInt16BE(0), Buffer.alloc(3, 1).toJSON().data.join(""));
console.log(Buffer.from("ff00", "hex").length, Buffer.from("aGk=", "base64").toString(), b.slice(1, 3).toString(), b.subarray(1).length, JSON.stringify(Buffer.from([1, 2])), b.indexOf("wörld"), b.includes("x"), Buffer.compare(Buffer.from("a"), Buffer.from("b")));
const w = Buffer.alloc(8); w.writeUInt32LE(0xdeadbeef, 0); w.writeInt16BE(-2, 4); console.log(w.readUInt32LE(0).toString(16), w.readInt16BE(4), w.readUInt8(4), w.toString("hex"));
const f = Buffer.alloc(8); f.writeDoubleBE(1.5, 0); console.log(f.readDoubleBE(0), f.toString("hex"), f.writeFloatLE(2.5, 0), f.readFloatLE(0));
const g = Buffer.alloc(8); g.writeBigUInt64BE(0x0102030405060708n); console.log(g.readBigUInt64BE(), g.readBigInt64LE(), g.toString("hex"));
const s = b.subarray(0, 2); s[0] = 65; console.log(b.toString().slice(0, 1), b.fill("ab").toString(), Buffer.from("abc").toString("latin1", 1), new Uint8Array([1, 2]).toString(), typeof new Uint8Array(1).equals);
console.log(Buffer.from(new Uint8Array([9, 8])).equals(Buffer.from([9, 8])), Buffer.from(b.buffer, 2, 3).length, Buffer.byteLength("aGk=", "base64"), Buffer.isEncoding("utf8"), Buffer.isEncoding("nope"), Buffer.from("x").write("yz", 0), Buffer.alloc(2).fill(0x41).toString());
console.log(Buffer.from("hi", "utf16le").toString("hex"), Buffer.alloc(4, "ab").toString(), Buffer("xy").toString(), Buffer.from("ff", "hex").toString("hex"));
