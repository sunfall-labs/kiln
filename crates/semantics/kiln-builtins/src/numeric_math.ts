// Math.f16round is a TypeScript facade over the existing Float16Array element conversion.
(function installMathNumericBuiltins() {
  const Float16 = (globalThis as any).Float16Array;
  if (typeof Float16 !== "function") return;

  // TypedArray element conversion is the engine's ToFloat16 implementation, including ties to
  // even, overflow, subnormals, NaN and signed zero. A private scratch element avoids an
  // allocation per call. ToNumber runs before touching it, so recursive coercion is safe.
  const scratch = new Float16(1);
  const f16round = { f16round(x: any): number {
    scratch[0] = +x;
    return scratch[0];
  } }.f16round;
  Object.defineProperty(Math, "f16round", {
    value: f16round,
    writable: true,
    enumerable: false,
    configurable: true,
  });
})();
