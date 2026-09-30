// kiln-builtins: node:util implementation (Phase 5.2, ADR 0005)

const customInspectSymbol = Symbol.for("nodejs.util.inspect.custom");
const customPromisifySymbol = Symbol.for("nodejs.util.promisify.custom");

export const TextDecoder = globalThis.TextDecoder;
export const TextEncoder = globalThis.TextEncoder;

export function inspect(value: any, options?: any): string {
  if (value === undefined) return "undefined";
  if (value === null) return "null";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "number") return Object.is(value, -0) ? "-0" : String(value);
  if (typeof value === "bigint") return `${value}n`;
  if (typeof value === "string") return JSON.stringify(value);
  if (typeof value === "symbol") return value.toString();
  if (typeof value === "function") {
    return value.name ? `[Function: ${value.name}]` : "[Function (anonymous)]";
  }
  if (typeof value !== "object") {
    return String(value);
  }

  // Custom inspection method
  if (typeof value[customInspectSymbol] === "function") {
    return String(value[customInspectSymbol](2, options));
  }

  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    const items: string[] = [];
    for (let i = 0; i < value.length; i++) {
      const item = value[i];
      items.push(item === value ? "[Circular]" : inspect(item, options));
    }
    return `[ ${items.join(", ")} ]`;
  }

  if (value instanceof Date) {
    return value.toISOString();
  }
  if (value instanceof RegExp) {
    return value.toString();
  }
  if (value instanceof Error) {
    return value.stack || `${value.name}: ${value.message}`;
  }
  if (ArrayBuffer.isView(value) && !(value instanceof DataView)) {
    const ctorName = value.constructor ? value.constructor.name : "TypedArray";
    const arr = Array.from(value as any);
    return `${ctorName}(${arr.length}) [ ${arr.join(", ")} ]`;
  }

  const keys = Object.keys(value);
  if (keys.length === 0) return "{}";
  const entries: string[] = [];
  for (let i = 0; i < keys.length; i++) {
    const k = keys[i];
    const v = value[k];
    const valStr = v === value ? "[Circular]" : inspect(v, options);
    entries.push(`${k}: ${valStr}`);
  }
  return `{ ${entries.join(", ")} }`;
}

inspect.custom = customInspectSymbol;

export function formatWithOptions(inspectOptions: any, ...args: any[]): string {
  if (args.length === 0) {
    return "";
  }
  const first = args[0];
  if (typeof first !== "string") {
    return args.map((a) => inspect(a, inspectOptions)).join(" ");
  }
  if (args.length === 1) {
    return first;
  }

  let out = "";
  let argIndex = 1;
  const len = first.length;
  let i = 0;

  while (i < len) {
    const ch = first.charCodeAt(i);
    if (ch !== 37) {
      // '%'
      out += first[i];
      i++;
      continue;
    }

    if (i + 1 >= len) {
      out += "%";
      break;
    }

    const nextCh = first[i + 1];
    if (nextCh === "%") {
      out += "%";
      i += 2;
      continue;
    }

    if (argIndex >= args.length) {
      out += "%" + nextCh;
      i += 2;
      continue;
    }

    const val = args[argIndex];
    if (nextCh === "s") {
      out += String(val);
      argIndex++;
      i += 2;
    } else if (nextCh === "d" || nextCh === "i") {
      out += Number(val).toString();
      argIndex++;
      i += 2;
    } else if (nextCh === "f") {
      out += Number(val).toString();
      argIndex++;
      i += 2;
    } else if (nextCh === "j") {
      try {
        out += JSON.stringify(val);
      } catch {
        out += "[Circular]";
      }
      argIndex++;
      i += 2;
    } else if (nextCh === "o" || nextCh === "O") {
      out += inspect(val, inspectOptions);
      argIndex++;
      i += 2;
    } else {
      out += "%" + nextCh;
      i += 2;
    }
  }

  while (argIndex < args.length) {
    out += " " + inspect(args[argIndex++], inspectOptions);
  }

  return out;
}

export function format(...args: any[]): string {
  return formatWithOptions(undefined, ...args);
}

export function stripVTControlCharacters(value: string): string {
  if (typeof value !== "string") {
    throw new TypeError("The \"str\" argument must be of type string");
  }
  // CSI sequences and OSC sequences (terminated by BEL or ST) are the control forms used by
  // terminal styling libraries and command line package help output.
  return value.replace(/\u001B\[[0-?]*[ -/]*[@-~]|\u001B\][^\u0007]*(?:\u0007|\u001B\\)/g, "");
}

export function isDeepStrictEqual(actual: any, expected: any): boolean {
  const leftSeen: any[] = [];
  const rightSeen: any[] = [];
  function unorderedMatch(left: any[], right: any[], matches: (a: any, b: any) => boolean): boolean {
    if (left.length !== right.length) return false;
    const used: boolean[] = [];
    function search(index: number): boolean {
      if (index === left.length) return true;
      for (let candidate = 0; candidate < right.length; candidate++) {
        if (used[candidate]) continue;
        const leftMark = leftSeen.length;
        const rightMark = rightSeen.length;
        if (matches(left[index], right[candidate])) {
          used[candidate] = true;
          if (search(index + 1)) return true;
          used[candidate] = false;
        }
        leftSeen.length = leftMark;
        rightSeen.length = rightMark;
      }
      return false;
    }
    return search(0);
  }
  function equal(left: any, right: any): boolean {
    if (Object.is(left, right)) return true;
    if (left === null || right === null || typeof left !== "object" || typeof right !== "object") {
      return false;
    }
    if (Object.getPrototypeOf(left) !== Object.getPrototypeOf(right)) return false;
    for (let i = 0; i < leftSeen.length; i++) {
      if (leftSeen[i] === left) return rightSeen[i] === right;
      if (rightSeen[i] === right) return false;
    }
    leftSeen.push(left);
    rightSeen.push(right);

    if (left instanceof Date || right instanceof Date) {
      if (!(left instanceof Date) || !(right instanceof Date) || !Object.is(left.getTime(), right.getTime())) return false;
    }
    if (left instanceof RegExp || right instanceof RegExp) {
      if (!(left instanceof RegExp) || !(right instanceof RegExp) ||
          left.source !== right.source || left.flags !== right.flags || left.lastIndex !== right.lastIndex) return false;
    }
    if (ArrayBuffer.isView(left) || ArrayBuffer.isView(right)) {
      if (!ArrayBuffer.isView(left) || !ArrayBuffer.isView(right) ||
          left.constructor !== right.constructor || left.byteLength !== right.byteLength) return false;
      const a = new Uint8Array(left.buffer, left.byteOffset, left.byteLength);
      const b = new Uint8Array(right.buffer, right.byteOffset, right.byteLength);
      for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    }
    if (left instanceof ArrayBuffer || right instanceof ArrayBuffer) {
      if (!(left instanceof ArrayBuffer) || !(right instanceof ArrayBuffer) || left.byteLength !== right.byteLength) return false;
      const a = new Uint8Array(left), b = new Uint8Array(right);
      for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
    }
    if (Array.isArray(left) !== Array.isArray(right)) return false;
    if (Array.isArray(left) && left.length !== right.length) return false;
    if (left instanceof Map || right instanceof Map) {
      if (!(left instanceof Map) || !(right instanceof Map) || left.size !== right.size) return false;
      const a = Array.from(left.entries()), b = Array.from(right.entries());
      if (!unorderedMatch(a, b, (x, y) => equal(x[0], y[0]) && equal(x[1], y[1]))) return false;
    }
    if (left instanceof Set || right instanceof Set) {
      if (!(left instanceof Set) || !(right instanceof Set) || left.size !== right.size) return false;
      const a = Array.from(left.values()), b = Array.from(right.values());
      if (!unorderedMatch(a, b, equal)) return false;
    }
    const aKeys = Reflect.ownKeys(left).filter((key: any) => Object.prototype.propertyIsEnumerable.call(left, key));
    const bKeys = Reflect.ownKeys(right).filter((key: any) => Object.prototype.propertyIsEnumerable.call(right, key));
    if (aKeys.length !== bKeys.length) return false;
    for (let i = 0; i < aKeys.length; i++) {
      const key = aKeys[i];
      if (!Object.prototype.propertyIsEnumerable.call(right, key) || !equal(left[key], right[key])) return false;
    }
    return true;
  }
  return equal(actual, expected);
}

export function inherits(ctor: any, superCtor: any): void {
  if (typeof ctor !== "function" || typeof superCtor !== "function") {
    throw new TypeError("The constructor and super constructor must be functions");
  }
  if (superCtor.prototype === undefined) {
    throw new TypeError("The super constructor to \"inherits\" must have a prototype");
  }
  ctor.super_ = superCtor;
  Object.setPrototypeOf(ctor.prototype, superCtor.prototype);
}

export function deprecate<T extends (...args: any[]) => any>(
  fn: T,
  _msg?: string,
  _code?: string
): T {
  let warned = false;
  function deprecated(this: any, ...args: any[]) {
    if (!warned) {
      warned = true;
    }
    return fn.apply(this, args);
  }
  Object.setPrototypeOf(deprecated, fn);
  if (fn.prototype) {
    deprecated.prototype = fn.prototype;
  }
  return deprecated as T;
}

export function debuglog(_section: string, callback?: any): (...args: any[]) => void {
  function debug(..._args: any[]) {
    if (typeof callback === "function") {
      callback(debug);
    }
  }
  return debug;
}

export function promisify(original: any): any {
  if (typeof original !== "function") {
    throw new TypeError("The \"original\" argument must be of type Function");
  }
  if (original[customPromisifySymbol]) {
    const fn = original[customPromisifySymbol];
    if (typeof fn !== "function") {
      throw new TypeError("The \"customPromisify\" property must be of type Function");
    }
    return fn;
  }
  function fn(this: any, ...args: any[]) {
    return new Promise((resolve, reject) => {
      original.call(this, ...args, (err: any, ...values: any[]) => {
        if (err) return reject(err);
        if (values.length <= 1) return resolve(values[0]);
        resolve(values);
      });
    });
  }
  Object.setPrototypeOf(fn, Object.getPrototypeOf(original));
  return fn;
}

promisify.custom = customPromisifySymbol;

export function callbackify(
  original: (...args: any[]) => Promise<any>
): (...args: any[]) => void {
  if (typeof original !== "function") {
    throw new TypeError("The \"original\" argument must be of type Function");
  }
  function callbackified(this: any, ...args: any[]) {
    const maybeCb = args.pop();
    if (typeof maybeCb !== "function") {
      throw new TypeError("The last argument must be of type Function");
    }
    const cb = (...cbArgs: any[]) => {
      maybeCb.apply(this, cbArgs);
    };
    original.apply(this, args).then(
      (ret) => queueMicrotask(() => cb(null, ret)),
      (err) => queueMicrotask(() => cb(err))
    );
  }
  Object.setPrototypeOf(callbackified, Object.getPrototypeOf(original));
  return callbackified;
}


export const types = {
  isAnyArrayBuffer: (val: any): boolean => val instanceof ArrayBuffer,
  isArgumentsObject: (val: any): boolean =>
    Object.prototype.toString.call(val) === "[object Arguments]",
  isArrayBuffer: (val: any): boolean => val instanceof ArrayBuffer,
  isAsyncFunction: (val: any): boolean =>
    Object.prototype.toString.call(val) === "[object AsyncFunction]",
  isBooleanObject: (val: any): boolean =>
    Object.prototype.toString.call(val) === "[object Boolean]",
  isBoxedPrimitive: (val: any): boolean =>
    typeof val === "object" &&
    val !== null &&
    (val instanceof Boolean || val instanceof Number || val instanceof String),
  isDataView: (val: any): boolean => val instanceof DataView,
  isDate: (val: any): boolean => val instanceof Date,
  isGeneratorFunction: (val: any): boolean =>
    Object.prototype.toString.call(val) === "[object GeneratorFunction]",
  isMap: (val: any): boolean => val instanceof Map,
  isNativeError: (val: any): boolean => val instanceof Error,
  isPromise: (val: any): boolean => val instanceof Promise,
  isRegExp: (val: any): boolean => val instanceof RegExp,
  isSet: (val: any): boolean => val instanceof Set,
  isTypedArray: (val: any): boolean =>
    ArrayBuffer.isView(val) && !(val instanceof DataView),
  isUint8Array: (val: any): boolean => val instanceof Uint8Array,
};

export function isArray(val: any): boolean {
  return Array.isArray(val);
}
export function isBoolean(val: any): boolean {
  return typeof val === "boolean";
}
export function isBuffer(val: any): boolean {
  return val != null && typeof val === "object" && val._isBuffer === true;
}
export function isDate(val: any): boolean {
  return val instanceof Date;
}
export function isError(val: any): boolean {
  return val instanceof Error;
}
export function isFunction(val: any): boolean {
  return typeof val === "function";
}
export function isNull(val: any): boolean {
  return val === null;
}
export function isNullOrUndefined(val: any): boolean {
  return val === null || val === undefined;
}
export function isNumber(val: any): boolean {
  return typeof val === "number";
}
export function isObject(val: any): boolean {
  return val !== null && typeof val === "object";
}
export function isPrimitive(val: any): boolean {
  return val === null || (typeof val !== "object" && typeof val !== "function");
}
export function isRegExp(val: any): boolean {
  return val instanceof RegExp;
}
export function isString(val: any): boolean {
  return typeof val === "string";
}
export function isSymbol(val: any): boolean {
  return typeof val === "symbol";
}
export function isUndefined(val: any): boolean {
  return val === undefined;
}

const ansiStyles: Record<string, [number, number]> = {
  reset: [0, 0],
  bold: [1, 22],
  dim: [2, 22],
  italic: [3, 23],
  underline: [4, 24],
  blink: [5, 25],
  inverse: [7, 27],
  hidden: [8, 28],
  strikethrough: [9, 29],
  black: [30, 39],
  red: [31, 39],
  green: [32, 39],
  yellow: [33, 39],
  blue: [34, 39],
  magenta: [35, 39],
  cyan: [36, 39],
  white: [37, 39],
  gray: [90, 39],
  grey: [90, 39],
  blackBright: [90, 39],
  redBright: [91, 39],
  greenBright: [92, 39],
  yellowBright: [93, 39],
  blueBright: [94, 39],
  magentaBright: [95, 39],
  cyanBright: [96, 39],
  whiteBright: [97, 39],
  bgBlack: [40, 49],
  bgRed: [41, 49],
  bgGreen: [42, 49],
  bgYellow: [43, 49],
  bgBlue: [44, 49],
  bgMagenta: [45, 49],
  bgCyan: [46, 49],
  bgWhite: [47, 49],
  bgGray: [100, 49],
  bgGrey: [100, 49],
  bgBlackBright: [100, 49],
  bgRedBright: [101, 49],
  bgGreenBright: [102, 49],
  bgYellowBright: [103, 49],
  bgBlueBright: [104, 49],
  bgMagentaBright: [105, 49],
  bgCyanBright: [106, 49],
  bgWhiteBright: [107, 49],
};

export function styleText(format: string | string[], text: string): string {
  const formats = Array.isArray(format) ? format : [format];
  let open = "";
  let close = "";
  for (let i = 0; i < formats.length; i++) {
    const pair = ansiStyles[formats[i]];
    if (pair) {
      open += `\x1b[${pair[0]}m`;
      close = `\x1b[${pair[1]}m` + close;
    }
  }
  return open + text + close;
}

const util = {
  TextDecoder,
  TextEncoder,
  callbackify,
  debuglog,
  deprecate,
  format,
  formatWithOptions,
  inherits,
  inspect,
  isArray,
  isBoolean,
  isBuffer,
  isDate,
  isError,
  isFunction,
  isNull,
  isNullOrUndefined,
  isNumber,
  isObject,
  isPrimitive,
  isRegExp,
  isString,
  isSymbol,
  isUndefined,
  isDeepStrictEqual,
  promisify,
  styleText,
  types,
  default: undefined as any,
};
util.default = util;

export default util;
