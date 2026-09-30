const values = new Float16Array([
  1,
  1.00048828125,
  1.0009765625,
  -0,
  65520,
  5.960464477539063e-8,
  2.9802322387695312e-8,
  NaN,
  Infinity,
]);
console.log(
  values.length,
  values.BYTES_PER_ELEMENT,
  values.byteLength,
  values[0],
  values[1],
  values[2],
  Object.is(values[3], -0),
  values[4],
  values[5],
  values[6],
  Number.isNaN(values[7]),
  values[8],
  values instanceof Float16Array,
);
const buffer = new ArrayBuffer(6);
const view = new Float16Array(buffer, 2, 2);
view[0] = 1.5;
view[1] = -2.25;
console.log(view.byteOffset, view.byteLength, view[0], view[1], new Uint8Array(buffer)[2], new Uint8Array(buffer)[3]);
