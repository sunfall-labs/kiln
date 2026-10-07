import { resolve } from "node:path";
import { pathToFileURL } from "node:url";

const nativeGetTime = Date.prototype.getTime;
const nativeSetTime = Date.prototype.setTime;
const names = ["setUTCHours", "setUTCMinutes", "setUTCSeconds", "setUTCMilliseconds"];
const nativeSetters = Object.fromEntries(names.map((name) => [name, Date.prototype[name]]));
await import(pathToFileURL(resolve(process.argv[2])).href);
const installedSetters = Object.fromEntries(names.map((name) => [name, Date.prototype[name]]));

let cases = 0;
let mismatches = 0;
function sameNumber(left, right) {
  return Object.is(left, right) || (Number.isNaN(left) && Number.isNaN(right));
}
function run(method, time, args, label) {
  const expectedDate = new Date(time);
  const actualDate = new Date(time);
  let expected, actual, expectedError, actualError;
  try { expected = nativeSetters[method].apply(expectedDate, args.expected); }
  catch (error) { expectedError = error?.name ?? typeof error; }
  try { actual = installedSetters[method].apply(actualDate, args.actual); }
  catch (error) { actualError = error?.name ?? typeof error; }
  const expectedTime = nativeGetTime.call(expectedDate);
  const actualTime = nativeGetTime.call(actualDate);
  const pass = expectedError === actualError &&
    (expectedError !== undefined || sameNumber(expected, actual)) &&
    sameNumber(expectedTime, actualTime) &&
    JSON.stringify(args.expectedLog ?? []) === JSON.stringify(args.actualLog ?? []);
  cases++;
  if (!pass) {
    mismatches++;
    console.error(JSON.stringify({ label, method, time, expected, actual, expectedError, actualError, expectedTime, actualTime, expectedLog: args.expectedLog, actualLog: args.actualLog }));
  }
  return pass;
}

const boundaryTimes = [-86400001, -86400000, -86399999, -1, 0, 1, 86399999, 86400000, 86400001, 8.64e15, -8.64e15];
const patterns = {
  setUTCHours: [[0], [24], [-1, 60], [1, undefined], [23, 59, 59, 999], [1.9, -0.9, 61.8, 1001.7], [Infinity], [NaN, 1, 2, 3]],
  setUTCMinutes: [[0], [60], [-1, 60], [1, undefined], [59, 59, 999], [1.9, -0.9, 1001.7], [Infinity], [NaN, 1, 2]],
  setUTCSeconds: [[0], [60], [-1, 1000], [1, undefined], [59, 999], [1.9, 1001.7], [Infinity], [NaN, 2]],
  setUTCMilliseconds: [[0], [1000], [-1], [undefined], [999], [1.9], [Infinity], [NaN]],
};
for (const method of names) {
  for (const time of boundaryTimes) for (const args of patterns[method]) run(method, time, { expected: args, actual: args }, `boundary-${method}`);
}

let seed = 0x4d595df4;
function random() {
  seed = (Math.imul(seed, 1664525) + 1013904223) >>> 0;
  return seed / 0x100000000;
}
const possible = [-100000.75, -1000.5, -1.9, -0, 0, 0.9, 1.8, 59.9, 999.7, 86400000, NaN, Infinity, -Infinity, undefined];
for (const method of names) {
  const maxArgs = { setUTCHours: 4, setUTCMinutes: 3, setUTCSeconds: 2, setUTCMilliseconds: 1 }[method];
  for (let i = 0; i < 256; i++) {
    const time = Math.trunc((random() * 2 - 1) * 8.64e15);
    const argc = 1 + Math.floor(random() * maxArgs);
    const args = Array.from({ length: argc }, () => possible[Math.floor(random() * possible.length)]);
    run(method, time, { expected: args, actual: args }, `random-${method}-${i}`);
  }
}

const mutationValues = {
  setUTCHours: [1, 2, 3, 4],
  setUTCMinutes: [5, 6, 7],
  setUTCSeconds: [8, 9],
  setUTCMilliseconds: [10],
};
for (const method of names) {
  const values = mutationValues[method];
  const expectedLog = [], actualLog = [];
  const expectedDate = new Date(-1);
  const actualDate = new Date(-1);
  const expectedArgs = values.map((value, i) => ({ valueOf() { expectedLog.push(i); nativeSetTime.call(expectedDate, 123456789); return value; } }));
  const actualArgs = values.map((value, i) => ({ valueOf() { actualLog.push(i); nativeSetTime.call(actualDate, 123456789); return value; } }));
  const expected = nativeSetters[method].apply(expectedDate, expectedArgs);
  const actual = installedSetters[method].apply(actualDate, actualArgs);
  const pass = sameNumber(expected, actual) && sameNumber(nativeGetTime.call(expectedDate), nativeGetTime.call(actualDate)) && JSON.stringify(expectedLog) === JSON.stringify(actualLog);
  cases++;
  if (!pass) {
    mismatches++;
    console.error(JSON.stringify({ label: "mutation-during-coercion", method, expected, actual, expectedTime: nativeGetTime.call(expectedDate), actualTime: nativeGetTime.call(actualDate), expectedLog, actualLog }));
  }
}

// Invalid time is captured before coercion. The conversion still runs and can mutate the receiver,
// but the time setter returns NaN without replacing that side effect.
for (const method of names) {
  const expectedDate = new Date(NaN), actualDate = new Date(NaN);
  const expectedLog = [], actualLog = [];
  const expectedArgs = [{ valueOf() { expectedLog.push("arg"); nativeSetTime.call(expectedDate, 1234); return 1; } }];
  const actualArgs = [{ valueOf() { actualLog.push("arg"); nativeSetTime.call(actualDate, 1234); return 1; } }];
  const expected = nativeSetters[method].apply(expectedDate, expectedArgs);
  const actual = installedSetters[method].apply(actualDate, actualArgs);
  const pass = Number.isNaN(expected) && Number.isNaN(actual) && nativeGetTime.call(expectedDate) === 1234 && nativeGetTime.call(actualDate) === 1234 && JSON.stringify(expectedLog) === JSON.stringify(actualLog);
  cases++;
  if (!pass) {
    mismatches++;
    console.error(JSON.stringify({ label: "invalid-mutation", method, expected, actual, expectedTime: nativeGetTime.call(expectedDate), actualTime: nativeGetTime.call(actualDate), expectedLog, actualLog }));
  }
}

console.log(`cases=${cases} mismatches=${mismatches}`);
if (mismatches !== 0) process.exitCode = 1;
