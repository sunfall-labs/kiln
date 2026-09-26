// kiln-builtins: node:util/types implementation (Phase 5.2, ADR 0005)

export function isAnyArrayBuffer(val: any): boolean {
  return val instanceof ArrayBuffer;
}

export function isArgumentsObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object Arguments]";
}

export function isArrayBuffer(val: any): boolean {
  return val instanceof ArrayBuffer;
}

export function isArrayBufferView(val: any): boolean {
  return ArrayBuffer.isView(val);
}

export function isAsyncFunction(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object AsyncFunction]";
}

export function isBigInt64Array(val: any): boolean {
  return typeof BigInt64Array !== "undefined" && val instanceof BigInt64Array;
}

export function isBigIntObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object BigInt]" && typeof val === "object";
}

export function isBigUint64Array(val: any): boolean {
  return typeof BigUint64Array !== "undefined" && val instanceof BigUint64Array;
}

export function isBooleanObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object Boolean]";
}

export function isBoxedPrimitive(val: any): boolean {
  return (
    typeof val === "object" &&
    val !== null &&
    (val instanceof Boolean || val instanceof Number || val instanceof String)
  );
}

export function isDataView(val: any): boolean {
  return val instanceof DataView;
}

export function isDate(val: any): boolean {
  return val instanceof Date;
}

export function isFloat32Array(val: any): boolean {
  return typeof Float32Array !== "undefined" && val instanceof Float32Array;
}

export function isFloat64Array(val: any): boolean {
  return typeof Float64Array !== "undefined" && val instanceof Float64Array;
}

export function isGeneratorFunction(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object GeneratorFunction]";
}

export function isInt8Array(val: any): boolean {
  return typeof Int8Array !== "undefined" && val instanceof Int8Array;
}

export function isInt16Array(val: any): boolean {
  return typeof Int16Array !== "undefined" && val instanceof Int16Array;
}

export function isInt32Array(val: any): boolean {
  return typeof Int32Array !== "undefined" && val instanceof Int32Array;
}

export function isMap(val: any): boolean {
  return val instanceof Map;
}

export function isNativeError(val: any): boolean {
  return val instanceof Error;
}

export function isNumberObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object Number]" && typeof val === "object";
}

export function isPromise(val: any): boolean {
  return val instanceof Promise;
}

export function isRegExp(val: any): boolean {
  return val instanceof RegExp;
}

export function isSet(val: any): boolean {
  return val instanceof Set;
}

export function isStringObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object String]" && typeof val === "object";
}

export function isSymbolObject(val: any): boolean {
  return Object.prototype.toString.call(val) === "[object Symbol]" && typeof val === "object";
}

export function isTypedArray(val: any): boolean {
  return ArrayBuffer.isView(val) && !(val instanceof DataView);
}

export function isUint8Array(val: any): boolean {
  return val instanceof Uint8Array;
}

export function isUint8ClampedArray(val: any): boolean {
  return typeof Uint8ClampedArray !== "undefined" && val instanceof Uint8ClampedArray;
}

export function isUint16Array(val: any): boolean {
  return typeof Uint16Array !== "undefined" && val instanceof Uint16Array;
}

export function isUint32Array(val: any): boolean {
  return typeof Uint32Array !== "undefined" && val instanceof Uint32Array;
}

export function isWeakMap(val: any): boolean {
  return typeof WeakMap !== "undefined" && val instanceof WeakMap;
}

export function isWeakSet(val: any): boolean {
  return typeof WeakSet !== "undefined" && val instanceof WeakSet;
}

const typesDefault = {
  isAnyArrayBuffer,
  isArgumentsObject,
  isArrayBuffer,
  isArrayBufferView,
  isAsyncFunction,
  isBigInt64Array,
  isBigIntObject,
  isBigUint64Array,
  isBooleanObject,
  isBoxedPrimitive,
  isDataView,
  isDate,
  isFloat32Array,
  isFloat64Array,
  isGeneratorFunction,
  isInt8Array,
  isInt16Array,
  isInt32Array,
  isMap,
  isNativeError,
  isNumberObject,
  isPromise,
  isRegExp,
  isSet,
  isStringObject,
  isSymbolObject,
  isTypedArray,
  isUint8Array,
  isUint8ClampedArray,
  isUint16Array,
  isUint32Array,
  isWeakMap,
  isWeakSet,
};

export default typesDefault;
