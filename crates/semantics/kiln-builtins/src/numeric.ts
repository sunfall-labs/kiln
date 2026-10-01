// Shared numeric built-ins whose answer is already represented by the engines' existing numeric
// storage and BigInt operators. Install them in TypeScript so interpreter and native execution
// share coercion order and descriptor behavior.
(function installNumericBuiltins() {
  const defineMethod = (target: any, name: string, value: any): void => {
    Object.defineProperty(target, name, {
      value,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  // TypedArray element conversion is the engine's ToFloat16 implementation, including ties to
  // even, overflow, subnormals, NaN and signed zero. A single private scratch element avoids an
  // allocation per call. ToNumber runs before touching it, so recursive coercion is safe.
  const Float16 = (globalThis as any).Float16Array;
  if (typeof Float16 === "function") {
    const scratch = new Float16(1);
    const f16round = { f16round(x: any): number {
      scratch[0] = +x;
      return scratch[0];
    } }.f16round;
    defineMethod(Math, "f16round", f16round);
  }

  if (typeof BigInt === "function") {
    const BigIntValue: any = BigInt;
    const MAX_SAFE = Number.MAX_SAFE_INTEGER;
    // ToIndex first performs abstract ToNumber (unary + deliberately rejects BigInt), then
    // truncates toward zero and checks the safe-integer range.
    const toIndex = (input: any): number => {
      const number = +input;
      if (number !== number || number === 0) return 0;
      const integer = number < 0 ? Math.ceil(number) : Math.floor(number);
      if (integer < 0 || integer > MAX_SAFE) {
        throw new RangeError("Invalid index");
      }
      return integer;
    };

    const truncate = (bitCount: any, input: any, signed: boolean): bigint => {
      // The specification converts bits before converting the value to BigInt.
      const bits = toIndex(bitCount);
      const value = BigIntValue(input);
      if (bits === 0) return 0n;

      const width = BigIntValue(bits);
      if (signed) {
        const upper = value >> (width - 1n);
        if ((value < 0n && upper === -1n) || (value >= 0n && upper === 0n)) return value;
      } else if (value >= 0n && (value >> width) === 0n) {
        return value;
      }

      // Match the interpreter BigInt representation authority (`truncate_bigint` in
      // intrinsic_helpers.rs): it caps materialized moduli at 2^24 bits. A negative value
      // converted to unsigned at a huge width must produce a correspondingly huge BigInt. Bound
      // that materialization rather than exhausting the process; ordinary wide inputs that
      // already fit returned above without allocating the modulus.
      if (bits > (1 << 24)) throw new RangeError("BigInt result exceeds the supported width");
      const modulus = 1n << width;
      let result = value % modulus;
      if (result < 0n) result += modulus;
      if (signed && result >= (modulus >> 1n)) result -= modulus;
      return result;
    };

    const asIntN = { asIntN(bits: any, value: any): bigint {
      return truncate(bits, value, true);
    } }.asIntN;
    const asUintN = { asUintN(bits: any, value: any): bigint {
      return truncate(bits, value, false);
    } }.asUintN;
    defineMethod(BigInt, "asIntN", asIntN);
    defineMethod(BigInt, "asUintN", asUintN);
  }
})();
