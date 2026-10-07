// `DataView`: the one view whose element type and byte order are chosen per access.
//
// The check this file exists for is the byte order. A `DataView` defaults to *big*-endian --
// the opposite of every typed array and of every machine this runs on -- so the same two bytes
// read one way through a view and the other way through a `Uint16Array` over the same buffer.
// Getting that backwards is invisible to a program that only writes and reads through one view,
// which is why every ordering check below reads the bytes back through a typed array as well.

const buffer = new ArrayBuffer(8);
const view = new DataView(buffer);
const bytes = new Uint8Array(buffer);

// The three constructor shapes, and the window each one describes.
console.log(view.byteLength, view.byteOffset, view.buffer === buffer);
const offsetView = new DataView(buffer, 2);
console.log(offsetView.byteLength, offsetView.byteOffset);
const boundedView = new DataView(buffer, 2, 3);
console.log(boundedView.byteLength, boundedView.byteOffset);
console.log(new DataView(buffer, 8).byteLength, new DataView(buffer, 0, 0).byteLength);

// BIG-ENDIAN IS THE DEFAULT. The first write puts the most significant byte at the lowest
// address; the second, asked for little-endian, puts it at the highest.
view.setUint16(0, 0x1234);
console.log(bytes[0], bytes[1], view.getUint16(0), view.getUint16(0, true));
view.setUint16(0, 0x1234, true);
console.log(bytes[0], bytes[1], view.getUint16(0), view.getUint16(0, true));

// A falsy final argument is the default rather than a third state.
view.setUint32(0, 0x01020304);
console.log(bytes[0], bytes[3], view.getUint32(0, false), view.getUint32(0, undefined));

// The signed and unsigned readings of the same bytes, at all three integer widths.
view.setUint8(0, 255);
view.setUint16(2, 0xffff);
view.setUint32(4, 0xffffffff);
console.log(view.getInt8(0), view.getUint8(0));
console.log(view.getInt16(2), view.getUint16(2));
console.log(view.getInt32(4), view.getUint32(4));

// A setter takes `ToUint32` and then the low bytes of the width, exactly as `u8[0] = 300` does.
view.setUint8(0, 300);
console.log(view.getUint8(0));
view.setInt8(1, -1);
console.log(view.getUint8(1));
view.setInt16(2, -2);
console.log(view.getUint16(2));

// Floats round-trip through both widths and both orders.
view.setFloat64(0, Math.PI);
console.log(view.getFloat64(0));
view.setFloat64(0, Math.PI, true);
console.log(view.getFloat64(0, true), view.getFloat64(0) === Math.PI);
view.setFloat32(0, 1.5);
console.log(view.getFloat32(0));
view.setFloat32(0, 0.1);
console.log(view.getFloat32(0));
view.setFloat64(0, -0);
console.log(Object.is(view.getFloat64(0), -0));
view.setFloat64(0, NaN);
console.log(Number.isNaN(view.getFloat64(0)));

// An unaligned offset is ordinary here and illegal for a typed array: a `DataView` has no
// element grid to sit on. Both the offset inside the window and the window's own start.
view.setFloat64(0, 0);
view.setUint32(1, 0x0a0b0c0d);
console.log(view.getUint32(1), bytes[0], bytes[1], bytes[4], bytes[5]);
const odd = new DataView(new ArrayBuffer(9), 1);
odd.setFloat64(0, 1.5);
console.log(odd.byteOffset, odd.byteLength, odd.getFloat64(0));

// The last legal access ends exactly at the final byte; one further is a RangeError, and so is
// a negative offset, which `ToIndex` refuses before any byte is touched.
const four = new DataView(new ArrayBuffer(4));
console.log(four.getUint8(3), four.getUint16(2), four.getUint32(0));
for (const attempt of [
  () => four.getUint8(4),
  () => four.getUint16(3),
  () => four.getUint32(1),
  () => four.getFloat64(0),
  () => four.getUint8(-1),
  () => four.setUint8(4, 0),
]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}

// A window that does not fit is a RangeError; a first argument that is not an ArrayBuffer is a
// TypeError, and the two are told apart before any offset is looked at.
for (const attempt of [
  () => new DataView(buffer, 9),
  () => new DataView(buffer, 2, 7),
  () => new DataView(buffer, 0, 9),
]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}
for (const attempt of [
  () => new DataView(new Uint8Array(4) as any),
  () => new DataView([] as any),
  () => new DataView(undefined as any),
  () => (DataView as any)(buffer),
]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}

// The buffer is borrowed rather than copied, which is the whole reason a program reaches for
// one: a write through the view is visible through every other view of the same bytes.
const shared = new ArrayBuffer(4);
const left = new DataView(shared);
const right = new Uint8Array(shared);
left.setUint32(0, 0xdeadbeef);
console.log(right[0], right[1], right[2], right[3]);
right[0] = 0;
console.log(left.getUint32(0));

// A view over part of a buffer sees only its own window, and offset zero means its own start.
const windowed = new DataView(shared, 1, 2);
windowed.setUint16(0, 0x0102);
console.log(right[0], right[1], right[2], right[3], windowed.getUint16(0));

// The BigInt pair, which exists because sixty-four bits do not fit in a double. Both readings
// of the same eight bytes, and both orders.
const wide = new DataView(new ArrayBuffer(8));
const wideBytes = new Uint8Array(wide.buffer);
wide.setBigUint64(0, 18446744073709551615n);
console.log(wide.getBigUint64(0), wide.getBigInt64(0));
wide.setBigInt64(0, -1n);
console.log(wide.getBigUint64(0), wide.getBigInt64(0));
wide.setBigInt64(0, 9007199254740993n);
console.log(wide.getBigInt64(0), Number(wide.getBigInt64(0)) === 9007199254740992);
wide.setBigInt64(0, -9223372036854775808n);
console.log(wide.getBigInt64(0), wide.getBigUint64(0));
wide.setBigUint64(0, 1n);
console.log(wideBytes[0], wideBytes[7], wide.getBigUint64(0, true));
wide.setBigUint64(0, 1n, true);
console.log(wideBytes[0], wideBytes[7], wide.getBigUint64(0, true));

// Both setters wrap rather than refuse, which is what makes `setBigUint64(0, -1n)` all ones.
wide.setBigUint64(0, -1n);
console.log(wide.getBigUint64(0));
wide.setBigInt64(0, 2n ** 200n);
console.log(wide.getBigUint64(0));
wide.setBigInt64(0, 2n ** 64n + 5n);
console.log(wide.getBigUint64(0));

// `ToBigInt` accepts far less than `ToNumber`: a number is refused rather than converted.
wide.setBigInt64(0, true as any);
console.log(wide.getBigInt64(0));
wide.setBigInt64(0, "42" as any);
console.log(wide.getBigInt64(0));
for (const attempt of [
  () => wide.setBigInt64(0, 1 as any),
  () => wide.setBigUint64(0, 1.5 as any),
  () => wide.setBigInt64(0, null as any),
  () => wide.setBigInt64(0, undefined as any),
]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}

// What the rest of the language says about one.
console.log(typeof DataView, typeof view, DataView.name, DataView.prototype.constructor === DataView);
console.log(view instanceof DataView, view.constructor === DataView);
console.log(ArrayBuffer.isView(view), ArrayBuffer.isView(buffer), ArrayBuffer.isView(bytes));
console.log(Object.prototype.toString.call(view), String(view));
console.log(Object.keys(view).length, Object.getOwnPropertyNames(view).length);
console.log(Boolean(view), view ? "truthy" : "falsy");
console.log(view);
console.log(boundedView);

// A prototype accessor borrowed onto the wrong receiver is a TypeError about the receiver,
// which is what tells "this is not a view" apart from "this offset is out of range".
for (const attempt of [
  () => DataView.prototype.getUint8.call({}, 0),
  () => DataView.prototype.getUint8.call(bytes as any, 0),
  () => DataView.prototype.setUint8.call(buffer as any, 0, 1),
]) {
  try { attempt(); console.log("no throw"); }
  catch (error: any) { console.log(error.constructor.name); }
}

// Float16 accessors are the DataView counterpart to the existing Float16Array conversion.
const half = new DataView(new ArrayBuffer(8));
for (const value of [1.5, -0, 65504, 1e-8, NaN, Infinity]) {
  (half as any).setFloat16(0, value, true);
  const read = (half as any).getFloat16(0, true);
  console.log(Number.isNaN(read) ? "NaN" : Object.is(read, -0) ? "-0" : read);
}
console.log(DataView.prototype.getFloat16.name, DataView.prototype.getFloat16.length,
  DataView.prototype.setFloat16.name, DataView.prototype.setFloat16.length);
for (const accessor of [DataView.prototype.getFloat16, DataView.prototype.setFloat16]) {
  try { new (accessor as any)(); console.log("constructible"); }
  catch (error: any) { console.log(error.constructor.name); }
}

// Brand validation precedes coercion; after a valid brand, offsets and values are converted in
// order, including for detached views. Nested calls also exercise the bootstrap's shared scratch.
const float16Order: string[] = [];
const float16Offset = { valueOf(): number { float16Order.push("offset"); return 0; } };
const float16Value = { valueOf(): number {
  float16Order.push("value");
  (half as any).setFloat16(2, 2.5, true);
  return 1.5;
} };
(half as any).setFloat16(float16Offset, float16Value, true);
console.log(float16Order.join(","), (half as any).getFloat16(0, true), (half as any).getFloat16(2, true));
float16Order.length = 0;
try { DataView.prototype.getFloat16.call({}, float16Offset); }
catch (error: any) { console.log(error.constructor.name, float16Order.length); }
try { DataView.prototype.setFloat16.call({}, float16Offset, float16Value); }
catch (error: any) { console.log(error.constructor.name, float16Order.length); }
const detachedHalfBuffer = new ArrayBuffer(2);
const detachedHalf = new DataView(detachedHalfBuffer);
structuredClone(detachedHalfBuffer, { transfer: [detachedHalfBuffer] });
try { (detachedHalf as any).setFloat16(float16Offset, float16Value, true); }
catch (error: any) { console.log(float16Order.slice(-2).join(","), error.constructor.name); }
