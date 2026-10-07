// Reference instants were produced by Node's built-in Date, independently of kiln's shared TS.
const cases = [
  [-1, "setUTCHours", [3, 4, 5, 6]],
  [-86400001, "setUTCMinutes", [-61, 70, -3]],
  [8640000000000000, "setUTCMilliseconds", [1]],
  [-8640000000000000, "setUTCSeconds", [-1]],
  [0, "setUTCHours", [-25, 90, -61, 1001]],
  [-1, "setUTCSeconds", [-0.75, -0.75]],
  [123456789, "setUTCMinutes", [1.9, 2.9, 3.9]],
  [-123456789, "setUTCMilliseconds", [-1001.9]],
];
const expected = [-75354994, -93590003, NaN, NaN, -84659999, -60000, 122462003, -123458001];
for (let i = 0; i < cases.length; i++) {
  const [time, method, args] = cases[i];
  const date = new Date(time);
  const result = date[method](...args);
  if (!Object.is(result, expected[i]) || !Object.is(date.getTime(), expected[i])) {
    throw new Error(method + " reference mismatch at " + i + ": " + result);
  }
}
const date = new Date(-1);
const order = [];
const value = (name, number) => ({ valueOf() { order.push(name); date.setTime(0); return number; } });
const written = date.setUTCHours(value("hour", 3), value("minute", 4), value("second", 5), value("ms", 6));
if (written !== -75354994 || order.join(",") !== "hour,minute,second,ms") throw new Error("coercion mutation changed captured time");
const invalid = new Date(NaN);
const invalidOrder = [];
invalid.setUTCMinutes({ valueOf() { invalidOrder.push("minute"); return 1; } }, { valueOf() { invalidOrder.push("second"); return 2; } });
if (invalidOrder.join(",") !== "minute,second" || !Number.isNaN(invalid.getTime())) throw new Error("invalid Date skipped coercion");
for (const method of ["setUTCHours", "setUTCMinutes", "setUTCSeconds", "setUTCMilliseconds"]) {
  const d = new Date(123);
  let threw = false;
  try { d[method](1n); } catch (e) { threw = e instanceof TypeError; }
  if (!threw || d.getTime() !== 123) throw new Error("BigInt was accepted by " + method);
}
console.log("utc clock setters", cases.length, written, order.join(","), invalidOrder.join(","));
