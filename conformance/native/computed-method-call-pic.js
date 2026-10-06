// A computed method call site's cache is keyed by both receiver shape and property key.
const left = Symbol("left");
const right = Symbol("right");
const verifyReceiver = Symbol("verifyReceiver");
const receiver = {
  label: "receiver",
  [left]() { return `left:${this.label}`; },
  [right]() { return `right:${this.label}`; },
  [verifyReceiver]() { return this === proxied; },
};
let selected = left;
console.log(receiver[selected]());
selected = right;
console.log(receiver[selected]());
selected = left;
console.log(receiver[selected]());
const firstName = "first";
const secondName = "second";
const alternatingStrings = {
  [firstName]() { return `first:${this.label}`; },
  [secondName]() { return `second:${this.label}`; },
};
let stringName = firstName;
console.log(alternatingStrings[stringName]());
stringName = secondName;
console.log(alternatingStrings[stringName]());
stringName = firstName;
console.log(alternatingStrings[stringName]());
const keyNameCollision = {
  "@@iterator"() { return "string key"; },
  [Symbol.iterator]() { return "symbol key"; },
};
console.log(keyNameCollision["@@iterator"](), keyNameCollision[Symbol.iterator]());

// A mutation on the prototype must invalidate the remembered inherited callee.
const proto = {
  run() { return `old:${this.label}`; },
};
const child = Object.create(proto);
child.label = "child";
const run = "run";
console.log(child[run]());
proto.run = function () { return `new:${this.label}`; };
console.log(child[run]());

// Accessors and proxies keep their observable lookup and receiver behavior.
let getterReads = 0;
const accessor = {
  label: "accessor",
  get [left]() {
    getterReads += 1;
    const owner = this;
    return function () { return `${owner.label}:${getterReads}`; };
  },
};
console.log(accessor[left](), accessor[left](), getterReads);
let proxyReads = 0;
const proxied = new Proxy(receiver, {
  get(target, key, receiverValue) {
    proxyReads += 1;
    return Reflect.get(target, key, receiverValue);
  },
});
console.log(proxied[left](), proxied[verifyReceiver](), proxyReads);

// The value-call helper must keep the generic fallback for callable proxies, bound functions,
// and generators; only ordinary closures use its direct native invocation path.
const proxyCallableReceiver = {};
const proxyCallable = new Proxy(function (value) { return [this, value]; }, {
  apply(_target, thisValue, args) { return thisValue === proxyCallableReceiver && args[0] === 7; },
});
proxyCallableReceiver.invoke = proxyCallable;
console.log("proxy callable", proxyCallableReceiver["invoke"](7));
function boundTarget() { return this.label; }
const boundReceiver = { invoke: boundTarget.bind({ label: "bound receiver" }) };
console.log("bound computed", boundReceiver["invoke"]());
const generatorReceiver = {
  *iterate() { yield this === generatorReceiver; },
};
console.log("generator computed", generatorReceiver["iterate"]().next().value);

// Non-primitive keys still run ToPropertyKey exactly once before the call.
let conversions = 0;
const keyObject = {
  [Symbol.toPrimitive](hint) {
    conversions += 1;
    return hint === "string" ? "call" : "wrong";
  },
};
const coercible = { label: "coercible", call() { return this.label; } };
console.log(coercible[keyObject](), conversions);

// A computed call retains ToPropertyKey's result for both lookup and call-site metadata. The
// conversion, accessor and arguments run in that order; the accessor's captured callee survives
// mutations performed while evaluating the argument.
const computedCallOrder = [];
const computedAliasKey = {
  [Symbol.toPrimitive](hint) {
    computedCallOrder.push(`key:${hint}`);
    return "resolvedAlias";
  },
};
const computedAliasReceiver = {
  get resolvedAlias() {
    computedCallOrder.push("get");
    return function () { return this === computedAliasReceiver; };
  },
};
const computedAliasCallResult = computedAliasReceiver[computedAliasKey](
  computedCallOrder.push("argument"),
);
console.log("computed call order", computedAliasCallResult, computedCallOrder.join(","));

// Method metadata receives the canonical string produced by ToPropertyKey, even though the
// original computed key was an object. Keep the function anonymous so the frame needs that alias.
const computedAliasDataMethod = function () { throw new Error("computed alias"); };
Object.defineProperty(computedAliasDataMethod, "name", { value: "", configurable: true });
const computedAliasDataReceiver = { resolvedAlias: computedAliasDataMethod };
const priorPrepareStackTrace = Error.prepareStackTrace;
Error.prepareStackTrace = (_error, frames) =>
  frames.map((frame) => frame.getMethodName() ?? "").join(",");
try {
  computedAliasDataReceiver[computedAliasKey]();
} catch (error) {
  console.log("computed method alias", String(error.stack).split(",").includes("resolvedAlias"));
} finally {
  Error.prepareStackTrace = priorPrepareStackTrace;
}
let nullReceiverKeyConversions = 0;
try {
  null[{ [Symbol.toPrimitive]() { nullReceiverKeyConversions += 1; return "x"; } }]();
} catch (_error) {
  console.log("null receiver key conversions", nullReceiverKeyConversions);
}
let computedGetterArgumentRan = false;
try {
  const throwingGetter = Object.defineProperty({}, "run", {
    get() { throw new Error("getter before argument"); },
  });
  throwingGetter["run"]((computedGetterArgumentRan = true));
} catch (_error) {
  console.log("computed getter suppresses argument", computedGetterArgumentRan);
}
let computedNoncallableArgumentRan = false;
try {
  ({ run: 1 })["run"]((computedNoncallableArgumentRan = true));
} catch (error) {
  console.log("computed noncallable", error instanceof TypeError, computedNoncallableArgumentRan);
}

const capturedMethod = Symbol("capturedMethod");
const capturesCallee = {
  [capturedMethod]() { return "original"; },
};
function replaceDuringArgument() {
  capturesCallee[capturedMethod] = function () { return "replacement"; };
  return "argument";
}
console.log(capturesCallee[capturedMethod](replaceDuringArgument()));

// IteratorClose also uses a prepared named call: `return` is read once, tested, and then
// invoked with the iterator as its receiver.
let closeReads = 0;
let closeCalls = 0;
const closeIterator = {
  [Symbol.iterator]() { return this; },
  next() { return { value: 1, done: false }; },
  get return() {
    closeReads += 1;
    return function () {
      closeCalls += 1;
      return {};
    };
  },
};
for (const _ of closeIterator) break;
console.log(closeReads, closeCalls);

const orderKey = Symbol("order");
const order = [];
const orderedCall = {
  version: 1,
  get [orderKey]() {
    order.push("get");
    const version = this.version;
    return function (argument) { return `${version}:${argument}`; };
  },
};
function orderedArgument() {
  order.push("argument");
  orderedCall.version = 2;
  return "value";
}
console.log(orderedCall[orderKey](orderedArgument()), order.join(","));

// Named and computed calls both capture the callee before their arguments run.
const namedOrder = [];
const namedCall = {
  get run() {
    namedOrder.push("get");
    return function (argument) { return `captured:${argument}`; };
  },
};
function namedArgument() {
  namedOrder.push("argument");
  Object.defineProperty(namedCall, "run", {
    value() { return "replacement"; }, configurable: true,
  });
  return "value";
}
console.log(namedCall.run(namedArgument()), namedOrder.join(","));

// A saved closure with a numeric typed entry may still take that entry when its body identity,
// argument facts, and builtin guards match. The ordinary body remains the fallback.
const numericCall = {
  sum(left, right) { return left + right; },
};
console.log("typed", numericCall.sum(20, 22));

// A numeric intrinsic is authorized by the saved method ID, even if an argument replaces the
// live property after the call has already fetched its callee.
const builtinImul = Math.imul;
function replaceImulDuringArgument() {
  Math.imul = function () { return -1; };
  return 6;
}
console.log("saved intrinsic", Math.imul(7, replaceImulDuringArgument()));
Math.imul = builtinImul;
Math.imul = function () { return "overridden"; };
console.log("overridden intrinsic", Math.imul(2, 3) + 1);
Math.imul = builtinImul;

const punctuationKey = "quote\" slash\\ newline\n";
const punctuationMethods = {
  "quote\" slash\\ newline\n"() { return "punctuation method"; },
};
console.log("punctuation method", punctuationMethods[punctuationKey]());

let noncallableArgumentRan = false;
try {
  ({ run: 1 }).run((noncallableArgumentRan = true));
} catch (error) {
  console.log(error instanceof TypeError, noncallableArgumentRan);
}

const failingKey = Symbol("failing");
const failingReceiver = {
  [failingKey]() { throw new Error("computed symbol failure"); },
};
try {
  failingReceiver[failingKey]();
} catch (error) {
  console.log(error.name, error.message, error.stack.includes("computed-method-call-pic.js"));
}
