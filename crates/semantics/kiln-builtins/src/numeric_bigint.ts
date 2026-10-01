// BigInt.asIntN/asUintN over the engines' existing arbitrary-precision BigInt operators.
(function installBigIntNumericBuiltins() {
  if (typeof BigInt !== "function") return;
  const BigIntValue: any = BigInt;
  const apply = Reflect.apply;
  const floor = Math.floor;
  const ceil = Math.ceil;
  const numberIsNaN = Number.isNaN;
  const MAX_SAFE = Number.MAX_SAFE_INTEGER;
  const toPrimitiveSymbol = Symbol.toPrimitive;

  // Abstract ToPrimitive with the number hint, shared by ToBigInt and ordinary ToNumber.
  // `Reflect.apply` is captured before user code can replace it, and preserves the receiver.
  const toPrimitive = (input: any): any => {
    if (input === null || (typeof input !== "object" && typeof input !== "function")) {
      return input;
    }
    const exotic = input[toPrimitiveSymbol];
    if (exotic !== undefined && exotic !== null) {
      if (typeof exotic !== "function") throw new TypeError("Cannot convert object to primitive value");
      const result = apply(exotic, input, ["number"]);
      if (result === null || (typeof result !== "object" && typeof result !== "function")) {
        return result;
      }
      throw new TypeError("Cannot convert object to primitive value");
    }
    const valueOf = input.valueOf;
    if (typeof valueOf === "function") {
      const result = apply(valueOf, input, []);
      if (result === null || (typeof result !== "object" && typeof result !== "function")) return result;
    }
    const toString = input.toString;
    if (typeof toString === "function") {
      const result = apply(toString, input, []);
      if (result === null || (typeof result !== "object" && typeof result !== "function")) return result;
    }
    throw new TypeError("Cannot convert object to primitive value");
  };

  // ToIndex: ToNumber is intentionally unary plus, which rejects BigInt and Symbol where the
  // explicit Number() function would accept BigInt. Fractional values truncate toward zero.
  const toIndex = (input: any): number => {
    const number = +toPrimitive(input);
    if (numberIsNaN(number) || number === 0) return 0;
    const integer = number < 0 ? ceil(number) : floor(number);
    if (integer < 0 || integer > MAX_SAFE) throw new RangeError("Invalid index");
    return integer;
  };

  // Abstract ToBigInt: first ToPrimitive with number hint, then reject Numbers (including a
  // Number returned by an object's coercion hook). BigInt(string) provides the specification's
  // string grammar; the explicit boolean arm handles false/true as 0n/1n.
  const toBigInt = (input: any): bigint => {
    const primitive = toPrimitive(input);
    if (typeof primitive === "bigint") return primitive;
    if (typeof primitive === "boolean") return primitive ? 1n : 0n;
    if (typeof primitive === "string") return BigIntValue(primitive);
    throw new TypeError("Cannot convert value to a BigInt");
  };

  const truncate = (bitCount: any, input: any, signed: boolean): bigint => {
    // The specification converts bits before converting the value to BigInt, even when bits is 0.
    const bits = toIndex(bitCount);
    const value = toBigInt(input);
    if (bits === 0) return 0n;

    const width = BigIntValue(bits);
    if (signed) {
      const upper = value >> (width - 1n);
      if ((value < 0n && upper === -1n) || (value >= 0n && upper === 0n)) return value;
    } else if (value >= 0n && (value >> width) === 0n) {
      return value;
    }

    // Match the interpreter BigInt representation authority (`truncate_bigint` in
    // intrinsic_helpers.rs): it caps materialized moduli at 2^24 bits. Wide values which already
    // fit returned above without allocating the modulus.
    if (bits > (1 << 24)) throw new RangeError("BigInt result exceeds the supported width");
    const modulus = 1n << width;
    let result = value % modulus;
    if (result < 0n) result += modulus;
    if (signed && result >= (modulus >> 1n)) result -= modulus;
    return result;
  };

  const defineMethod = (name: string, value: any): void => {
    Object.defineProperty(BigInt, name, {
      value,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };
  defineMethod("asIntN", { asIntN(bits: any, value: any): bigint {
    return truncate(bits, value, true);
  } }.asIntN);
  defineMethod("asUintN", { asUintN(bits: any, value: any): bigint {
    return truncate(bits, value, false);
  } }.asUintN);
})();
