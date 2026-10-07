const d = new DataView(new ArrayBuffer(4));
console.log(typeof (DataView.prototype as any).getUint8);
console.log((DataView.prototype as any).getUint8.call(d, 0));
