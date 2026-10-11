// A single dynamic-write site sees changing strings, symbols, shapes and descriptor state.
// This keeps the computed-write cache behind the same ordinary [[Set]] semantics as its fallback.
// Run with scripts/native-realms.sh too: one emitted keyed site is shared while each realm
// observes its own shape and object state.
function check(condition, message) {
  if (!condition) throw new Error(message);
}

function keyFrom(parts) {
  return parts.join("");
}

function writeAt(target, key, value) {
  "use strict";
  return target[key] = value;
}

function sloppyWriteAt(target, key, value) {
  return target[key] = value;
}

const keys = [keyFrom(["al", "pha"]), keyFrom(["be", "ta"])];
const target = { alpha: 0, beta: 0 };
check(writeAt(target, keys[0], 11) === 11, "assignment returns its value");
check(writeAt(target, keys[1], 22) === 22, "same site distinguishes keys on one shape");
check(writeAt(target, keyFrom(["al", "pha"]), 33) === 33, "equal-content dynamic key hits the same slot");
check(target.alpha === 33 && target.beta === 22, "alternating keys update their own slots");

const symbol = Symbol("alpha");
check(writeAt(target, symbol, 44) === 44, "symbol assignment shares the site safely");
check(target.alpha === 33 && target[symbol] === 44, "symbol and same-description string remain distinct");

const unicodeTarget = {};
const astral = String.fromCodePoint(0x1f48e);
const loneSurrogate = String.fromCharCode(0xd800);
check(writeAt(unicodeTarget, astral, 51) === 51, "astral key assignment");
check(writeAt(unicodeTarget, loneSurrogate, 52) === 52, "lone-surrogate key assignment");
check(unicodeTarget[astral] === 51 && unicodeTarget[loneSurrogate] === 52,
  "full UTF-16 key contents remain distinct");

const sealed = { slot: 0 };
Object.seal(sealed);
check(writeAt(sealed, keyFrom(["sl", "ot"]), 61) === 61 && sealed.slot === 61,
  "sealed own writable data property remains assignable");
const nonExtensible = { slot: 0 };
Object.preventExtensions(nonExtensible);
check(writeAt(nonExtensible, keyFrom(["sl", "ot"]), 62) === 62 && nonExtensible.slot === 62,
  "non-extensible own writable data property remains assignable");

let setterValue = 0;
const reconfigured = { slot: 1 };
writeAt(reconfigured, keyFrom(["sl", "ot"]), 2);
Object.defineProperty(reconfigured, "slot", {
  configurable: true,
  set(value) { setterValue = value; },
});
writeAt(reconfigured, keyFrom(["sl", "ot"]), 71);
check(setterValue === 71, "data-to-accessor reconfiguration invokes setter");
Object.defineProperty(reconfigured, "slot", {
  configurable: true,
  writable: false,
  value: 2,
});
let rejected = false;
try { writeAt(reconfigured, keyFrom(["sl", "ot"]), 72); }
catch (error) { rejected = error instanceof TypeError; }
check(rejected && reconfigured.slot === 2, "non-writable reconfiguration keeps strict assignment semantics");
const sloppyNonWritable = {};
Object.defineProperty(sloppyNonWritable, "slot", { value: 5, writable: false });
check(sloppyWriteAt(sloppyNonWritable, "slot", 6) === 6 && sloppyNonWritable.slot === 5,
  "sloppy assignment returns its value without changing a non-writable property");

const deleted = { slot: 0 };
writeAt(deleted, keyFrom(["sl", "ot"]), 3);
delete deleted.slot;
deleted.slot = 4;
check(writeAt(deleted, keyFrom(["sl", "ot"]), 73) === 73 && deleted.slot === 73,
  "delete and reinsert do not use a stale slot");

let inheritedValue = 0;
const prototype = {
  set inherited(value) { inheritedValue = value; },
};
const child = Object.create(prototype);
writeAt(child, keyFrom(["in", "herited"]), 81);
check(inheritedValue === 81 && !Object.hasOwn(child, "inherited"),
  "inherited setter remains on the ordinary path");

let prototypeSetterValue = 0;
const changedPrototypeTarget = { slot: 0 };
writeAt(changedPrototypeTarget, keyFrom(["sl", "ot"]), 6);
Object.setPrototypeOf(changedPrototypeTarget, {
  set slot(value) { prototypeSetterValue = value; },
});
check(writeAt(changedPrototypeTarget, keyFrom(["sl", "ot"]), 82) === 82,
  "own slot remains writable after prototype replacement");
check(changedPrototypeTarget.slot === 82 && prototypeSetterValue === 0,
  "prototype replacement does not redirect an existing own slot");

const mutationKey = keyFrom(["mut", "a", "ble"]);
let mutationSetterCalls = 0;
const mutationPrototype = {};
Object.defineProperty(mutationPrototype, mutationKey, {
  configurable: true,
  set(value) {
    mutationSetterCalls++;
    // Keep the computed key live while the setter allocates and changes the receiver shape.
    let churn = null;
    for (let index = 0; index < 96; index++) churn = { index, previous: churn };
    Object.defineProperty(this, mutationKey, {
      configurable: true,
      enumerable: true,
      writable: true,
      value,
    });
    Object.defineProperty(this, "side", {
      configurable: true,
      enumerable: true,
      writable: true,
      value: churn.index,
    });
    Object.setPrototypeOf(this, null);
  },
});
const mutationTarget = Object.create(mutationPrototype);
check(writeAt(mutationTarget, keyFrom(["mu", "table"]), 111) === 111,
  "allocating inherited setter completes the first assignment");
check(mutationSetterCalls === 1 && Object.hasOwn(mutationTarget, mutationKey) &&
  mutationTarget[mutationKey] === 111 && mutationTarget.side === 95,
  "post-set lookup observes the receiver's setter-mutated shape");
check(writeAt(mutationTarget, keyFrom(["mutable"]), 112) === 112 &&
  mutationTarget[mutationKey] === 112 && mutationSetterCalls === 1,
  "learned slot remains valid after allocating setter and prototype mutation");

const frozen = { slot: 0 };
writeAt(frozen, keyFrom(["sl", "ot"]), 5);
Object.freeze(frozen);
rejected = false;
try { writeAt(frozen, keyFrom(["sl", "ot"]), 91); }
catch (error) { rejected = error instanceof TypeError; }
check(rejected && frozen.slot === 5, "freeze invalidates the writable-slot fast path");

let proxyKey;
const proxyTarget = { slot: 0 };
const proxy = new Proxy(proxyTarget, {
  set(target, key, value, receiver) {
    proxyKey = key;
    return Reflect.set(target, key, value, receiver);
  },
});
writeAt(proxy, keyFrom(["sl", "ot"]), 101);
check(proxyKey === "slot" && proxyTarget.slot === 101, "proxy set trap is preserved");

console.log("computed string own-write PIC semantics pass");
