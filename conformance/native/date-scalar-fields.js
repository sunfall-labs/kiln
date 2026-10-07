// Scalar Date fields must preserve full TimeClip range, invalid receivers and positive zero.
const methods = ["getUTCFullYear", "getUTCMonth", "getUTCDate", "getUTCDay", "getUTCHours", "getUTCMinutes", "getUTCSeconds", "getUTCMilliseconds", "getYear"];
const times = [NaN, Infinity, -Infinity, 0, -0, 1, -1, 999, -999, 1000, -1000, 59999, -59999, 60000, -60000, 86399999, -86399999, 86400000, -86400000, -950400000, 8.64e15, -8.64e15, 8.64e15 + 1, -8.64e15 - 1];
let seed = 1;
for (let i = 0; i < 200; i++) {
  seed = (seed * 1664525 + 1013904223) >>> 0;
  times.push(Math.trunc((seed / 4294967296 * 2 - 1) * 8.64e15));
}
let checksum = 0;
let fields = 0;
let invalid = 0;
for (const time of times) {
  const date = new Date(time);
  for (const method of methods) {
    const value = date[method]();
    if (Object.is(value, -0)) throw new Error(method + " returned negative zero");
    if (Number.isNaN(value)) invalid++;
    else checksum = ((checksum * 31) + value) | 0;
    fields++;
  }
}
let rejected = 0;
for (const method of methods) {
  try { Date.prototype[method].call({}); }
  catch (error) { if (error instanceof TypeError) rejected++; else throw error; }
}
console.log("fields", fields, "invalid", invalid, "checksum", checksum, "rejected", rejected);
