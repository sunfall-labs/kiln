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
const FastBuffer = Buffer[Symbol.species];
const bufferSpeciesDescriptor = Object.getOwnPropertyDescriptor(Buffer, Symbol.species);
if (typeof FastBuffer !== "function" || FastBuffer.name !== "FastBuffer" || FastBuffer === Buffer) throw new Error("Buffer species identity");
const speciesPrototypeDescriptor = Object.getOwnPropertyDescriptor(FastBuffer, "prototype");
if (FastBuffer.length !== 0 || speciesPrototypeDescriptor.writable || speciesPrototypeDescriptor.configurable || speciesPrototypeDescriptor.enumerable) throw new Error("FastBuffer class constructor metadata");
if (Object.getPrototypeOf(FastBuffer) !== Uint8Array || FastBuffer.prototype !== Buffer.prototype || Object.getPrototypeOf(FastBuffer.prototype) !== Uint8Array.prototype) throw new Error("Buffer species prototype chain");
const speciesBacking = new ArrayBuffer(5);
const speciesView = new FastBuffer(speciesBacking, 1, 3);
speciesView[0] = 71;
const speciesSized = new FastBuffer(2);
const speciesCoerced = new FastBuffer("2");
const speciesCopied = new FastBuffer(new Uint16Array([257, 258]));
const speciesSlice = Buffer.from(speciesBacking).subarray(1, 4);
if (!Buffer.isBuffer(speciesView) || speciesView.buffer !== speciesBacking || speciesView.byteOffset !== 1 || speciesBackingBytes(speciesBacking)[1] !== 71) throw new Error("FastBuffer must return a shared Buffer view");
if (!Buffer.isBuffer(speciesSized) || speciesSized.length !== 2 || speciesCoerced.length !== 2 || speciesCoerced[0] !== 0 || speciesCopied.toString("hex") !== "0102" || !Buffer.isBuffer(speciesSlice)) throw new Error("Buffer typed-array species construction");
class DerivedFastBuffer extends FastBuffer {}
const derivedSpecies = new DerivedFastBuffer(1);
if (!Buffer.isBuffer(derivedSpecies) || !(derivedSpecies instanceof DerivedFastBuffer)) throw new Error("FastBuffer subclass construction");

Object.defineProperty(Buffer, Symbol.species, {
  value: Uint8Array,
  configurable: true,
});
const ordinarySpeciesMap = Buffer.from([1, 2]).map(value => value + 1);
if (Object.getPrototypeOf(ordinarySpeciesMap) !== Uint8Array.prototype ||
    Buffer.isBuffer(ordinarySpeciesMap) || ordinarySpeciesMap.join(",") !== "2,3") {
  throw new Error("Buffer map must honor a replacement typed-array species");
}
class DerivedTypedArray extends Uint8Array {}
Object.defineProperty(Buffer, Symbol.species, {
  value: DerivedTypedArray,
  configurable: true,
});
const derivedSpeciesMap = Buffer.from([3]).filter(() => true);
if (!(derivedSpeciesMap instanceof DerivedTypedArray) || Buffer.isBuffer(derivedSpeciesMap) ||
    derivedSpeciesMap[0] !== 3) {
  throw new Error("Buffer filter must preserve custom species identity");
}
Object.defineProperty(Buffer, Symbol.species, bufferSpeciesDescriptor);
function speciesBackingBytes(buffer) { return new Uint8Array(buffer); }
