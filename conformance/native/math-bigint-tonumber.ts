// Math.* functions, isNaN, and isFinite convert arguments via ToNumber.
// In ECMA-262 §7.1.4 ToNumber, BigInt and Symbol throw TypeError.
// Number(x) converts via ToNumeric, so BigInt is converted to Number, while Symbol throws TypeError.

for (const [name, fn] of [
  ["sqrt", () => Math.sqrt(10n)],
  ["floor", () => Math.floor(1n)],
  ["max", () => Math.max(1n, 2)],
  ["abs", () => Math.abs(-1n)],
  ["atan2", () => Math.atan2(10n, 1)],
  ["hypot", () => Math.hypot(3n, 4)],
  ["imul", () => Math.imul(2n, 3)],
  ["isNaN", () => isNaN(10n)],
  ["isFinite", () => isFinite(10n)],
  ["Number", () => Number(10n)],
  ["sqrt-obj", () => Math.sqrt({ valueOf() { return 10n; } })],
  ["sqrt-sym", () => Math.sqrt(Symbol("x"))],
  ["sqrt-ok", () => Math.sqrt({ valueOf() { return 16; } })],
  ["min-nan", () => Math.min(NaN, 10n)],
  ["number-sym", () => Number(Symbol("s"))],
]) {
  try {
    console.log(name, fn());
  } catch (e) {
    console.log(name, "throws", e.constructor.name);
  }
}
