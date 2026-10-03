// kiln-builtins: node:assert implementation (Phase 5.2, ADR 0005)

export class AssertionError extends Error {
  actual: any;
  expected: any;
  operator: string;
  generatedMessage: boolean;
  code: string;

  constructor(options?: any) {
    let msg: string;
    let act: any = undefined;
    let exp: any = undefined;
    let op: string = "";
    let gen: boolean = false;

    if (typeof options === "string") {
      msg = options;
    } else if (options && typeof options === "object") {
      if (options.message !== undefined) {
        msg = String(options.message);
      } else {
        gen = true;
        msg = "The expression evaluated to a falsy value";
      }
      act = options.actual;
      exp = options.expected;
      op = options.operator !== undefined ? String(options.operator) : "";
    } else {
      gen = true;
      msg = "The expression evaluated to a falsy value";
    }

    super(msg);
    this.name = "AssertionError";
    this.code = "ERR_ASSERTION";
    this.actual = act;
    this.expected = exp;
    this.operator = op;
    this.generatedMessage = gen;
  }
}

function matchesExpectedError(err: any, expected: any): boolean {
  if (expected === undefined) {
    return true;
  }
  if (expected instanceof RegExp) {
    return expected.test(String(err && err.message ? err.message : err));
  }
  if (typeof expected === "function") {
    if (expected.prototype && err instanceof expected) {
      return true;
    }
    return Boolean(expected(err));
  }
  if (typeof expected === "object" && expected !== null) {
    for (const key of Object.keys(expected)) {
      if (err[key] !== expected[key]) {
        return false;
      }
    }
    return true;
  }
  return false;
}

type ComparedPairs = { left: any[]; right: any[] };

function isDeepEqual(
  a: any,
  b: any,
  strict: boolean,
  compared: ComparedPairs,
): boolean {
  if (strict ? Object.is(a, b) : a == b) {
    return true;
  }
  if (a === null || typeof a !== "object" || b === null || typeof b !== "object") {
    return false;
  }
  if (Array.isArray(a) !== Array.isArray(b)) {
    return false;
  }
  // Deep equality is defined over object graphs, not just trees. Remember
  // object pairs before descending so self references and mutual cycles end.
  for (let i = 0; i < compared.left.length; i++) {
    if (compared.left[i] === a && compared.right[i] === b) {
      return true;
    }
  }
  compared.left.push(a);
  compared.right.push(b);
  if (Array.isArray(a)) {
    if (a.length !== b.length) {
      return false;
    }
    for (let i = 0; i < a.length; i++) {
      if (!isDeepEqual(a[i], b[i], strict, compared)) {
        return false;
      }
    }
    return true;
  }
  const aKeys = Object.keys(a);
  const bKeys = Object.keys(b);
  if (aKeys.length !== bKeys.length) {
    return false;
  }
  for (let i = 0; i < aKeys.length; i++) {
    const key = aKeys[i];
    if (!Object.prototype.hasOwnProperty.call(b, key)) {
      return false;
    }
    if (!isDeepEqual(a[key], b[key], strict, compared)) {
      return false;
    }
  }
  return true;
}

function deepEqualGraph(a: any, b: any, strict: boolean): boolean {
  return isDeepEqual(a, b, strict, { left: [], right: [] });
}

export function fail(
  actual?: any,
  expected?: any,
  message?: any,
  operator?: string,
): never {
  if (arguments.length === 0) {
    throw new AssertionError({ message: "Failed", operator: "fail" });
  }
  if (arguments.length === 1) {
    if (actual instanceof Error) {
      throw actual;
    }
    throw new AssertionError({
      message: actual !== undefined ? String(actual) : "Failed",
      operator: "fail",
    });
  }
  if (message instanceof Error) {
    throw message;
  }
  throw new AssertionError({
    message: message !== undefined ? String(message) : "Failed",
    actual,
    expected,
    operator: operator || "fail",
  });
}

export function ok(value?: any, message?: any): void {
  if (!value) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : "The expression evaluated to a falsy value";
    throw new AssertionError({
      message: msg,
      actual: value,
      expected: true,
      operator: "==",
    });
  }
}

export function equal(actual: any, expected: any, message?: any): void {
  if (actual != expected) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined ? String(message) : `${actual} == ${expected}`;
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "==",
    });
  }
}

export function notEqual(actual: any, expected: any, message?: any): void {
  if (actual == expected) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined ? String(message) : `${actual} != ${expected}`;
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "!=",
    });
  }
}

export function strictEqual(actual: any, expected: any, message?: any): void {
  if (!Object.is(actual, expected)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : `Expected values to be strictly equal:\n\n${actual} !== ${expected}\n`;
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "strictEqual",
    });
  }
}

export function notStrictEqual(actual: any, expected: any, message?: any): void {
  if (Object.is(actual, expected)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : `Expected values to not be strictly equal:\n\n${actual} === ${expected}\n`;
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "notStrictEqual",
    });
  }
}

export function deepEqual(actual: any, expected: any, message?: any): void {
  if (!deepEqualGraph(actual, expected, false)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : "Expected values to be deeply equal";
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "deepEqual",
    });
  }
}

export function notDeepEqual(actual: any, expected: any, message?: any): void {
  if (deepEqualGraph(actual, expected, false)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : "Expected values not to be deeply equal";
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "notDeepEqual",
    });
  }
}

export function deepStrictEqual(actual: any, expected: any, message?: any): void {
  if (!deepEqualGraph(actual, expected, true)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : "Expected values to be deeply strictly equal";
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "deepStrictEqual",
    });
  }
}

export function notDeepStrictEqual(actual: any, expected: any, message?: any): void {
  if (deepEqualGraph(actual, expected, true)) {
    if (message instanceof Error) {
      throw message;
    }
    const msg =
      message !== undefined
        ? String(message)
        : "Expected values not to be deeply strictly equal";
    throw new AssertionError({
      message: msg,
      actual,
      expected,
      operator: "notDeepStrictEqual",
    });
  }
}

export function match(string: string, regexp: RegExp, message?: any): void {
  if (!(regexp instanceof RegExp)) {
    throw new TypeError("The \"regexp\" argument must be an instance of RegExp");
  }
  const str = String(string);
  if (!regexp.test(str)) {
    const msg =
      message !== undefined
        ? String(message)
        : `The input did not match the regular expression ${regexp}. Input:\n\n${str}\n`;
    throw new AssertionError({
      message: msg,
      actual: str,
      expected: regexp,
      operator: "match",
    });
  }
}

export function doesNotMatch(string: string, regexp: RegExp, message?: any): void {
  if (!(regexp instanceof RegExp)) {
    throw new TypeError("The \"regexp\" argument must be an instance of RegExp");
  }
  const str = String(string);
  if (regexp.test(str)) {
    const msg =
      message !== undefined
        ? String(message)
        : `The input was expected to not match the regular expression ${regexp}. Input:\n\n${str}\n`;
    throw new AssertionError({
      message: msg,
      actual: str,
      expected: regexp,
      operator: "doesNotMatch",
    });
  }
}

export function throws(fn: Function, expected?: any, message?: any): void {
  if (typeof fn !== "function") {
    throw new TypeError("The \"fn\" argument must be of type function");
  }
  let threw = false;
  let caught: any;
  try {
    fn();
  } catch (err) {
    threw = true;
    caught = err;
  }
  if (!threw) {
    throw new AssertionError({
      message:
        message !== undefined ? String(message) : "Missing expected exception",
      operator: "throws",
    });
  }
  if (expected !== undefined && !matchesExpectedError(caught, expected)) {
    throw caught;
  }
}

export function doesNotThrow(fn: Function, expected?: any, message?: any): void {
  if (typeof fn !== "function") {
    throw new TypeError("The \"fn\" argument must be of type function");
  }
  try {
    fn();
  } catch (err) {
    if (expected === undefined || matchesExpectedError(err, expected)) {
      const msg =
        message !== undefined
          ? String(message)
          : `Got unwanted exception: ${err && err.message ? err.message : err}`;
      throw new AssertionError({
        message: msg,
        actual: err,
        operator: "doesNotThrow",
      });
    }
    throw err;
  }
}

export async function rejects(asyncFn: any, expected?: any, message?: any): Promise<void> {
  const promise = typeof asyncFn === "function" ? asyncFn() : asyncFn;
  if (!promise || typeof promise.then !== "function") {
    throw new TypeError(
      "The \"asyncFn\" argument must be an async function or a promise",
    );
  }
  let threw = false;
  let caught: any;
  try {
    await promise;
  } catch (err) {
    threw = true;
    caught = err;
  }
  if (!threw) {
    throw new AssertionError({
      message:
        message !== undefined ? String(message) : "Missing expected rejection",
      operator: "rejects",
    });
  }
  if (expected !== undefined && !matchesExpectedError(caught, expected)) {
    throw caught;
  }
}

export async function doesNotReject(asyncFn: any, expected?: any, message?: any): Promise<void> {
  const promise = typeof asyncFn === "function" ? asyncFn() : asyncFn;
  if (!promise || typeof promise.then !== "function") {
    throw new TypeError(
      "The \"asyncFn\" argument must be an async function or a promise",
    );
  }
  try {
    await promise;
  } catch (err) {
    if (expected === undefined || matchesExpectedError(err, expected)) {
      const msg =
        message !== undefined
          ? String(message)
          : `Got unwanted rejection: ${err && err.message ? err.message : err}`;
      throw new AssertionError({
        message: msg,
        actual: err,
        operator: "doesNotReject",
      });
    }
    throw err;
  }
}

export function ifError(value: any): void {
  if (value !== null && value !== undefined) {
    let message = "ifError got unwanted exception: ";
    if (typeof value === "object" && typeof value.message === "string") {
      if (value.message.length === 0 && value.constructor) {
        message += value.constructor.name;
      } else {
        message += value.message;
      }
    } else {
      message += String(value);
    }
    const newErr = new AssertionError({
      actual: value,
      expected: null,
      operator: "ifError",
      message,
    });
    const origStack = value && value.stack;
    if (typeof origStack === "string") {
      newErr.stack = origStack;
    }
    throw newErr;
  }
}

export function strict(value?: any, message?: any): void {
  ok(value, message);
}

function assert(value?: any, message?: any): void {
  ok(value, message);
}

assert.AssertionError = AssertionError;
assert.deepEqual = deepEqual;
assert.deepStrictEqual = deepStrictEqual;
assert.default = assert;
assert.doesNotMatch = doesNotMatch;
assert.doesNotReject = doesNotReject;
assert.doesNotThrow = doesNotThrow;
assert.equal = equal;
assert.fail = fail;
assert.ifError = ifError;
assert.match = match;
assert.notDeepEqual = notDeepEqual;
assert.notDeepStrictEqual = notDeepStrictEqual;
assert.notEqual = notEqual;
assert.notStrictEqual = notStrictEqual;
assert.ok = ok;
assert.rejects = rejects;
assert.strict = strict;
assert.strictEqual = strictEqual;
assert.throws = throws;

strict.AssertionError = AssertionError;
strict.deepEqual = deepStrictEqual;
strict.deepStrictEqual = deepStrictEqual;
strict.default = strict;
strict.doesNotMatch = doesNotMatch;
strict.doesNotReject = doesNotReject;
strict.doesNotThrow = doesNotThrow;
strict.equal = strictEqual;
strict.fail = fail;
strict.ifError = ifError;
strict.match = match;
strict.notDeepEqual = notDeepStrictEqual;
strict.notDeepStrictEqual = notDeepStrictEqual;
strict.notEqual = notStrictEqual;
strict.notStrictEqual = notStrictEqual;
strict.ok = ok;
strict.rejects = rejects;
strict.strict = strict;
strict.strictEqual = strictEqual;
strict.throws = throws;

export default assert;
