// ArrayBuffer.prototype.slice correctness and bounds testing across interpreter and native backend.

const buffer = new ArrayBuffer(8);
const view = new Uint8Array(buffer);
for (let i = 0; i < 8; i++) {
  view[i] = (i + 1) * 10;
}

function dump(buf) {
  const u = new Uint8Array(buf);
  return `${buf.byteLength}:[${Array.from(u).join(",")}]`;
}

// Full clone (0 args)
console.log("clone:", dump(buffer.slice()));

// Start only
console.log("start 2:", dump(buffer.slice(2)));
console.log("start neg:", dump(buffer.slice(-3)));
console.log("start huge:", dump(buffer.slice(100)));

// Start and end
console.log("range 2..5:", dump(buffer.slice(2, 5)));
console.log("range neg:", dump(buffer.slice(-5, -2)));
console.log("inverted:", dump(buffer.slice(5, 2)));

// Coercion & edge values
console.log("undef:", dump(buffer.slice(undefined, undefined)));
console.log("nan:", dump(buffer.slice(NaN, 4)));

// Empty buffer
const empty = new ArrayBuffer(0);
console.log("empty:", dump(empty.slice()));

// Independence / memory isolation
const sub = buffer.slice(1, 4);
const subView = new Uint8Array(sub);
subView[0] = 99;
console.log("original after sub edit:", dump(buffer));
console.log("sub after sub edit:", dump(sub));
view[1] = 77;
console.log("sub after orig edit:", dump(sub));
