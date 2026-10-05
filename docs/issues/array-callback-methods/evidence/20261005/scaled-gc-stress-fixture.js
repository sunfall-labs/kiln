function check(label, condition) {
  if (!condition) throw new Error("array callback bootstrap: " + label);
}

function throwsTypeError(label, action) {
  let caught;
  try {
    action();
  } catch (error) {
    caught = error;
  }
  check(label, caught instanceof TypeError);
}

// The callback methods use ordinary JS frames, not one host-language frame per callback level.
function nestedMap(depth) {
  if (depth === 0) return 42;
  return [0].map(() => nestedMap(depth - 1))[0];
}
function nestedReduce(depth) {
  if (depth === 0) return 42;
  return [0].reduce(() => nestedReduce(depth - 1), 0);
}
check("map callback depth", nestedMap(80) === 42);
check("reduce callback depth", nestedReduce(80) === 42);

// ToObject/LengthOfArrayLike precede IsCallable; ArraySpeciesCreate follows it.
const order = [];
const badCallbackArrayLike = {
  get length() { order.push("length"); return 0; },
  get constructor() { order.push("constructor"); return Array; },
};
throwsTypeError("empty map checks callback", () => Array.prototype.map.call(badCallbackArrayLike, 1));
check("map validates callback before species", order.join(",") === "length");

const invalidCallbackOrder = [];
const invalidCallbackArray = new Proxy([1], {
  get(target, key, receiver) {
    invalidCallbackOrder.push(String(key));
    return Reflect.get(target, key, receiver);
  },
});
throwsTypeError("array map checks callback before constructor", () =>
  Array.prototype.map.call(invalidCallbackArray, null));
check("callback error precedes ArraySpeciesCreate",
  invalidCallbackOrder.join(",") === "length");

// Array-like receivers do not consult constructor or @@species.
const arrayLike = { 0: 3, length: 1 };
Object.defineProperty(arrayLike, "constructor", {
  get() { throw new Error("array-like constructor must not be read"); },
});
const arrayLikeResult = Array.prototype.map.call(arrayLike, (value) => value + 1);
check("array-like result uses Array", Array.isArray(arrayLikeResult));
check("array-like result value", arrayLikeResult[0] === 4);

// HasProperty sees inherited indexed values and holes remain holes in map results.
const sparse = new Array(3);
sparse[0] = 2;
Object.setPrototypeOf(sparse, { 1: 5 });
const sparseResult = Array.prototype.map.call(sparse, (value) => value * 2);
check("map visits inherited index", sparseResult[0] === 4 && sparseResult[1] === 10);
check("map preserves absent index", sparseResult.length === 3 && !(2 in sparseResult));
const explicitUndefined = new Array(4);
explicitUndefined[1] = undefined;
let undefinedVisits = 0;
explicitUndefined.map((_value, index) => { undefinedVisits |= 1 << index; });
check("an own undefined index differs from a hole", undefinedVisits === 2);
let inheritedGetterCalls = 0;
Object.defineProperty(Array.prototype, 2, {
  configurable: true,
  get() { inheritedGetterCalls += 1; return 6; },
});
try {
  const inherited = new Array(4);
  const inheritedResult = inherited.map((value) => value * 2);
  check("non-enumerable inherited index is visited",
    inheritedGetterCalls === 1 && inheritedResult[2] === 12);
} finally {
  delete Array.prototype[2];
}
Array.prototype[1] = 99;
const isolatedSparse = new Array(3);
Object.setPrototypeOf(isolatedSparse, null);
check("a null array prototype does not fall back to Array.prototype", !(1 in isolatedSparse));
check("map respects a null array prototype",
  Array.prototype.map.call(isolatedSparse, (value) => value).length === 3);
delete Array.prototype[1];

const savedArrayPrototypeParentForNull = Object.getPrototypeOf(Array.prototype);
const savedObjectPrototypeParentForNull = Object.getPrototypeOf(Object.prototype);
try {
  Object.prototype[6] = 60;
  Object.setPrototypeOf(Array.prototype, Object.create(null));
  const noObjectPrototypeIndex = new Array(8);
  let excludedPrototypeVisits = 0;
  check("custom null Array.prototype parent excludes Object.prototype index",
    !(6 in noObjectPrototypeIndex));
  noObjectPrototypeIndex.map(() => { excludedPrototypeVisits += 1; });
  check("map excludes indices beyond the actual custom prototype chain",
    excludedPrototypeVisits === 0);
} finally {
  delete Object.prototype[6];
  Object.setPrototypeOf(Array.prototype, savedArrayPrototypeParentForNull);
  Object.setPrototypeOf(Object.prototype, savedObjectPrototypeParentForNull);
}

const savedArrayPrototypeParent = Object.getPrototypeOf(Array.prototype);
const indexedArrayPrototypeParent = { 7: 70 };
try {
  Object.setPrototypeOf(Array.prototype, indexedArrayPrototypeParent);
  const inheritedFromPrototypeParent = new Array(10);
  const seenFromPrototypeParent = [];
  inheritedFromPrototypeParent.map((_value, index) => seenFromPrototypeParent.push(index));
  check("map falls back for an indexed custom Array.prototype parent",
    seenFromPrototypeParent.join(",") === "7");
} finally {
  Object.setPrototypeOf(Array.prototype, savedArrayPrototypeParent);
}

const savedArrayPrototypeForProxy = Object.getPrototypeOf(Array.prototype);
let inheritedProxyHasCalls = 0;
let inheritedProxyGetCalls = 0;
try {
  Object.setPrototypeOf(Array.prototype, new Proxy({}, {
    has(_target, key) {
      inheritedProxyHasCalls += 1;
      return key === "9";
    },
    get(_target, key, receiver) {
      if (key === "9") inheritedProxyGetCalls += 1;
      if (key === "9") check("proxy get preserves the original array receiver", receiver === proxyParentArray);
      return key === "9" ? 90 : undefined;
    },
  }));
  const proxyParentArray = new Array(10);
  check("computed Get reaches a Proxy in the indexed prototype chain", proxyParentArray[9] === 90);
  check("computed Get uses one Proxy get trap", inheritedProxyGetCalls === 1);
  inheritedProxyHasCalls = 0;
  inheritedProxyGetCalls = 0;
  const proxyParentVisits = [];
  proxyParentArray.map((value, index) => proxyParentVisits.push(`${index}:${value}`));
  check("map observes a proxy in the indexed prototype chain",
    proxyParentVisits.join(",") === "9:90" && inheritedProxyHasCalls === 10 &&
    inheritedProxyGetCalls === 1);
} finally {
  Object.setPrototypeOf(Array.prototype, savedArrayPrototypeForProxy);
}

const savedObjectPrototypeParent = Object.getPrototypeOf(Object.prototype);
try {
  const indexedObjectPrototypeParent = Object.create(null);
  indexedObjectPrototypeParent[8] = 80;
  Object.setPrototypeOf(Object.prototype, indexedObjectPrototypeParent);
  const inheritedFromObjectPrototypeParent = new Array(10);
  const seenFromObjectPrototypeParent = [];
  inheritedFromObjectPrototypeParent.map((_value, index) => seenFromObjectPrototypeParent.push(index));
  check("map falls back for an indexed custom Object.prototype parent",
    seenFromObjectPrototypeParent.join(",") === "8");
} finally {
  Object.setPrototypeOf(Object.prototype, savedObjectPrototypeParent);
}

const shrinkingFromRight = [0, 1, 2, 3];
const shrinkingRightVisits = [];
shrinkingFromRight.reduceRight((_accumulator, _value, index) => {
  shrinkingRightVisits.push(index);
  if (index === 3) shrinkingFromRight.length = 1;
  return 0;
}, 0);
check("reduceRight rechecks lower indices after truncation",
  shrinkingRightVisits.join(",") === "3,0");

Object.defineProperty(Array.prototype, 5, {
  configurable: true, value: 50, writable: true,
});
try {
  const inheritedAfterTruncate = new Array(8);
  inheritedAfterTruncate[0] = 0;
  inheritedAfterTruncate[1] = 1;
  inheritedAfterTruncate[7] = 7;
  const inheritedRightVisits = [];
  inheritedAfterTruncate.reduceRight((_accumulator, _value, index) => {
    inheritedRightVisits.push(index);
    if (index === 7) inheritedAfterTruncate.length = 2;
    return 0;
  }, 0);
  check("reduceRight finds inherited indices after truncation",
    inheritedRightVisits.join(",") === "7,5,1,0");
} finally {
  delete Array.prototype[5];
}

Object.defineProperty(RegExp.prototype, "arrayCallbackInherited", {
  value: 17, writable: true, enumerable: true, configurable: true,
});
const regexpWithInherited = /array/;
check("RegExp own-property query excludes inherited keys",
  !Object.prototype.hasOwnProperty.call(regexpWithInherited, "arrayCallbackInherited"));
delete RegExp.prototype.arrayCallbackInherited;
let dateSetterValue = "before";
Object.defineProperty(Date.prototype, "arrayCallbackSetter", {
  get() { return dateSetterValue; },
  set(value) { dateSetterValue = value; },
  enumerable: true,
  configurable: true,
});
const dateWithInheritedSetter = new Date();
dateWithInheritedSetter.arrayCallbackSetter = "after";
check("Date inherited setter keeps the property off the instance",
  dateSetterValue === "after" &&
  !Object.prototype.hasOwnProperty.call(dateWithInheritedSetter, "arrayCallbackSetter"));
delete Date.prototype.arrayCallbackSetter;

let defaultDateSetterCalls = 0;
Object.defineProperty(Date.prototype, "arrayCallbackSetter", {
  set() { defaultDateSetterCalls += 1; }, configurable: true,
});
let customDateSetterReceiver;
const customDatePrototype = Object.create(null);
Object.defineProperty(customDatePrototype, "arrayCallbackSetter", {
  set(value) { customDateSetterReceiver = [this, value]; },
});
const customPrototypeDate = new Date();
Object.setPrototypeOf(customPrototypeDate, customDatePrototype);
customPrototypeDate.arrayCallbackSetter = "custom";
check("Date custom prototype setter receives the instance",
  customDateSetterReceiver[0] === customPrototypeDate && customDateSetterReceiver[1] === "custom");
let ownDateSetterReceiver;
const ownSetterDate = new Date();
Object.defineProperty(ownSetterDate, "arrayCallbackOwnSetter", {
  set(value) { ownDateSetterReceiver = [this, value]; },
});
ownSetterDate.arrayCallbackOwnSetter = "own-setter";
check("Date own setter receives the instance, not its storage sidecar",
  ownDateSetterReceiver[0] === ownSetterDate && ownDateSetterReceiver[1] === "own-setter");
function DatePrototypeFunction() {}
let functionPrototypeDateReceiver;
Object.defineProperty(DatePrototypeFunction, "arrayCallbackFunctionSetter", {
  set(value) { functionPrototypeDateReceiver = [this, value]; },
});
const functionPrototypeDate = new Date();
Object.setPrototypeOf(functionPrototypeDate, DatePrototypeFunction);
functionPrototypeDate.arrayCallbackFunctionSetter = "function-prototype";
check("Date function prototype setter receives the Date instance",
  functionPrototypeDateReceiver[0] === functionPrototypeDate &&
  functionPrototypeDateReceiver[1] === "function-prototype");
const nullPrototypeDate = new Date();
Object.setPrototypeOf(nullPrototypeDate, null);
nullPrototypeDate.arrayCallbackSetter = "own";
check("Date null prototype does not invoke Date.prototype setter",
  defaultDateSetterCalls === 0 &&
  Object.prototype.hasOwnProperty.call(nullPrototypeDate, "arrayCallbackSetter"));
let proxyDateSetRecord;
const proxyDatePrototypeTarget = {};
const proxyDatePrototype = new Proxy(proxyDatePrototypeTarget, {
  set(target, key, value, receiver) {
    proxyDateSetRecord = [target, key, value, receiver];
    return true;
  },
});
const proxyPrototypeDate = new Date();
Object.setPrototypeOf(proxyPrototypeDate, proxyDatePrototype);
proxyPrototypeDate.arrayCallbackProxySetter = "proxy";
check("Date custom Proxy prototype receives Set with the Date receiver",
  proxyDateSetRecord[0] === proxyDatePrototypeTarget &&
  proxyDateSetRecord[1] === "arrayCallbackProxySetter" &&
  proxyDateSetRecord[2] === "proxy" && proxyDateSetRecord[3] === proxyPrototypeDate);
delete Date.prototype.arrayCallbackSetter;

// Length is captured before callbacks; presence and values are re-read at each index.
const changing = [1, 2, 3];
const changingResult = changing.map((value, index) => {
  if (index === 0) {
    changing.length = 1;
    changing.push(9);
  }
  return value;
});
check("map snapshots length and rechecks presence",
  changingResult.length === 3 && changingResult[0] === 1 && changingResult[1] === 9 &&
  !(2 in changingResult));

const proxyHasKeys = [];
const proxyArray = new Proxy([, 2, ,], {
  has(target, key) {
    proxyHasKeys.push(String(key));
    return Reflect.has(target, key);
  },
});
proxyArray.map((value) => value);
check("proxy HasProperty traps run in ascending order",
  proxyHasKeys.join(",") === "0,1,2");

const millionSparse = new Array(1_000);
for (let index = 0; index < 6; index += 1) millionSparse[index] = index;
millionSparse[999] = -1;
const millionVisited = [];
millionSparse.map((_value, index) => { millionVisited.push(index); });
check("large sparse map visits only present slots",
  millionVisited.join(",") === "0,1,2,3,4,5,999");

// Species construction is observable before the first callback, and output writes are
// CreateDataProperty operations rather than assignments through inherited setters.
const speciesOrder = [];
let inheritedSetterCalls = 0;
function Result(length) {
  speciesOrder.push("construct:" + length);
  Object.setPrototypeOf(this, {
    set 0(_value) { inheritedSetterCalls += 1; },
  });
}
class SpeciesInput extends Array {
  static get [Symbol.species]() {
    speciesOrder.push("species");
    return Result;
  }
}
const speciesResult = new SpeciesInput(7, 8).map((value, index) => {
  speciesOrder.push("callback:" + index);
  return value + 1;
});
check("species order", speciesOrder.join(",") === "species,construct:2,callback:0,callback:1");
check("species result has mapped own properties",
  speciesResult[0] === 8 && speciesResult[1] === 9 &&
  Object.prototype.hasOwnProperty.call(speciesResult, "0"));
check("species property definition bypasses setters", inheritedSetterCalls === 0);

const definePropertySnapshots = [];
const descriptorProxy = new Proxy({}, {
  defineProperty(target, key, descriptor) {
    definePropertySnapshots.push({ key, descriptor });
    return true;
  },
});
const sourceDataDescriptor = {
  value: 11,
  writable: 0,
  enumerable: 1,
  configurable: 1,
};
Object.defineProperty(descriptorProxy, "data", sourceDataDescriptor);
const dataSnapshot = definePropertySnapshots[0].descriptor;
sourceDataDescriptor.value = 12;
check("proxy receives a fresh normalized data descriptor",
  dataSnapshot !== sourceDataDescriptor && dataSnapshot.value === 11 &&
  dataSnapshot.writable === false && dataSnapshot.enumerable === true &&
  dataSnapshot.configurable === true &&
  Reflect.ownKeys(dataSnapshot).join(",") === "value,writable,enumerable,configurable");
check("FromPropertyDescriptor fields are ordinary data properties",
  Object.getOwnPropertyDescriptor(dataSnapshot, "value").writable &&
  Object.getOwnPropertyDescriptor(dataSnapshot, "value").enumerable &&
  Object.getOwnPropertyDescriptor(dataSnapshot, "value").configurable);

const descriptorGetter = () => 21;
const descriptorSetter = (_value) => {};
const sourceAccessorDescriptor = {
  get: descriptorGetter,
  set: descriptorSetter,
  enumerable: 1,
  configurable: 1,
};
Object.defineProperty(descriptorProxy, "accessor", sourceAccessorDescriptor);
const accessorSnapshot = definePropertySnapshots[1].descriptor;
check("proxy receives only normalized accessor descriptor fields",
  accessorSnapshot !== sourceAccessorDescriptor &&
  accessorSnapshot.get === descriptorGetter && accessorSnapshot.set === descriptorSetter &&
  accessorSnapshot.enumerable === true && accessorSnapshot.configurable === true &&
  Reflect.ownKeys(accessorSnapshot).join(",") === "get,set,enumerable,configurable");

let descriptorSetReadAfterInvalidGet = false;
const invalidAccessorDescriptor = {};
Object.defineProperties(invalidAccessorDescriptor, {
  get: { get() { return 1; } },
  set: { get() { descriptorSetReadAfterInvalidGet = true; return descriptorSetter; } },
});
throwsTypeError("invalid descriptor getter is rejected", () =>
  Object.defineProperty(descriptorProxy, "invalid", invalidAccessorDescriptor));
check("descriptor getter validation precedes reading set", !descriptorSetReadAfterInvalidGet);

const descriptorReadOrder = [];
const orderedDescriptor = {};
for (const field of ["enumerable", "configurable", "get", "set"]) {
  Object.defineProperty(orderedDescriptor, field, {
    get() {
      descriptorReadOrder.push(field);
      return field === "get" || field === "set" ? undefined : 1;
    },
  });
}
Object.defineProperty(descriptorProxy, "ordered", orderedDescriptor);
check("ToPropertyDescriptor reads each field once in specification order",
  descriptorReadOrder.join(",") === "enumerable,configurable,get,set");
const dataDescriptorReadOrder = [];
const orderedDataDescriptor = {};
for (const field of ["enumerable", "configurable", "value", "writable"]) {
  Object.defineProperty(orderedDataDescriptor, field, {
    get() { dataDescriptorReadOrder.push(field); return field === "value" ? 23 : 1; },
  });
}
Object.defineProperty(descriptorProxy, "ordered-data", orderedDataDescriptor);
check("ToPropertyDescriptor reads data fields once in specification order",
  dataDescriptorReadOrder.join(",") === "enumerable,configurable,value,writable");

let groupedDescriptorReads = 0;
const groupedSourceDescriptor = { value: 31, enumerable: 1, configurable: 1 };
const definePropertiesDescriptors = {};
Object.defineProperty(definePropertiesDescriptors, "grouped", {
  enumerable: true,
  get() {
    groupedDescriptorReads += 1;
    return groupedSourceDescriptor;
  },
});
Object.defineProperties(descriptorProxy, definePropertiesDescriptors);
const definePropertiesSnapshot = definePropertySnapshots[4].descriptor;
check("defineProperties also supplies a FromPropertyDescriptor snapshot",
  definePropertiesSnapshot !== groupedSourceDescriptor &&
  definePropertiesSnapshot.value === 31 && definePropertiesSnapshot.enumerable === true &&
  definePropertiesSnapshot.configurable === true &&
  Reflect.ownKeys(definePropertiesSnapshot).join(",") === "value,enumerable,configurable");
check("defineProperties reads each descriptor-map value once", groupedDescriptorReads === 1);

let definePropertiesTrapCalls = 0;
const stagedDefinitionTarget = new Proxy({}, {
  defineProperty(target, key, descriptor) {
    definePropertiesTrapCalls += 1;
    return Reflect.defineProperty(target, key, descriptor);
  },
});
const stagedDefinitions = {
  first: { value: 1 },
  second: { get get() { return 1; } },
};
throwsTypeError("defineProperties converts every descriptor before defining", () =>
  Object.defineProperties(stagedDefinitionTarget, stagedDefinitions));
check("invalid later descriptor prevents earlier proxy trap", definePropertiesTrapCalls === 0);

const revokedDefinition = Proxy.revocable({}, {});
revokedDefinition.revoke();
let revokedDescriptorRead = false;
throwsTypeError("revoked proxy definition throws", () => Object.defineProperty(
  revokedDefinition.proxy,
  "x",
  { get enumerable() { revokedDescriptorRead = true; return true; } },
));
check("descriptor conversion precedes proxy revocation", revokedDescriptorRead);

const trapDescriptors = [];
function ProxyResult() {
  return new Proxy({}, {
    defineProperty(target, key, descriptor) {
      trapDescriptors.push(descriptor);
      return Reflect.defineProperty(target, key, descriptor);
    },
  });
}
class ProxySpeciesInput extends Array {
  static get [Symbol.species]() { return ProxyResult; }
}
new ProxySpeciesInput(4, 5).map((value) => value + 1);
check("proxy receives an independent descriptor snapshot for each index",
  trapDescriptors.length === 2 && trapDescriptors[0] !== trapDescriptors[1] &&
  trapDescriptors[0].value === 5 && trapDescriptors[1].value === 6);

const rejectingDefinitionTarget = new Proxy({}, { defineProperty() { return false; } });
throwsTypeError("Object.defineProperty rejects a false proxy result", () =>
  Object.defineProperty(rejectingDefinitionTarget, "x", { value: 1 }));
check("Reflect.defineProperty returns false for a false proxy result",
  Reflect.defineProperty(rejectingDefinitionTarget, "x", { value: 1 }) === false);
function RejectingSpecies() {
  return new Proxy({}, { defineProperty() { return false; } });
}
class RejectingSpeciesInput extends Array {
  static get [Symbol.species]() { return RejectingSpecies; }
}
throwsTypeError("map propagates a rejected proxy CreateDataProperty", () =>
  Object.assign(new RejectingSpeciesInput(1), { 0: 1 }).map((value) => value));

const closedDefineTarget = Object.preventExtensions({});
const lyingDefineProxy = new Proxy(closedDefineTarget, { defineProperty() { return true; } });
throwsTypeError("proxy cannot add to a non-extensible target", () =>
  Object.defineProperty(lyingDefineProxy, "new", { value: 1 }));
const fixedDefineTarget = {};
Object.defineProperty(fixedDefineTarget, "fixed", {
  value: 1,
  configurable: false,
  writable: false,
});
const lyingFixedDefineProxy = new Proxy(fixedDefineTarget, {
  defineProperty() { return true; },
});
throwsTypeError("proxy cannot report a changed non-configurable value", () =>
  Reflect.defineProperty(lyingFixedDefineProxy, "fixed", { value: 2 }));

const nullSpeciesInput = [1];
Object.defineProperty(nullSpeciesInput, "constructor", {
  value: { [Symbol.species]: null },
});
const nullSpeciesResult = nullSpeciesInput.map((value) => value + 1);
check("null species falls back to the current Array",
  Array.isArray(nullSpeciesResult) && nullSpeciesResult[0] === 2);

let constructorGetterCalls = 0;
const nullConstructorInput = [1];
Object.defineProperty(nullConstructorInput, "constructor", {
  get() { constructorGetterCalls += 1; return null; },
});
throwsTypeError("null constructor is not a species", () => nullConstructorInput.map(() => 0));
check("null constructor does not read species", constructorGetterCalls === 1);

const invalidSpeciesInput = [1];
Object.defineProperty(invalidSpeciesInput, "constructor", {
  value: { [Symbol.species]: {} },
});
let invalidSpeciesCallbacks = 0;
throwsTypeError("non-constructor species throws", () =>
  invalidSpeciesInput.map(() => { invalidSpeciesCallbacks += 1; return 0; }));
check("species validation precedes callbacks", invalidSpeciesCallbacks === 0);

const revokedSpecies = Proxy.revocable(function Species() {}, {});
const revokedSpeciesInput = [1];
revokedSpeciesInput.constructor = revokedSpecies.proxy;
revokedSpecies.revoke();
throwsTypeError("revoked species proxy throws during GetFunctionRealm", () =>
  revokedSpeciesInput.map(() => 0));

// The callback receiver is supplied by Call, while reduce/reduceRight use undefined.
const receiver = { marker: 10 };
const mappedThis = [1].map(function (value) { return value + this.marker; }, receiver);
check("map thisArg", mappedThis[0] === 11);
const strictReducer = function (accumulator, value) {
  "use strict";
  if (this !== undefined) throw new Error("reduce callback receiver");
  return accumulator + value;
};
check("reduce callback receiver", [1, 2].reduce(strictReducer, 0) === 3);

// Explicit undefined is an initial value. Without it, the first present element seeds the fold.
const inheritedReduce = new Array(3);
Object.setPrototypeOf(inheritedReduce, { 1: 5 });
check("reduce inherited seed", inheritedReduce.reduce((a, b) => a + b) === 5);
check("reduce explicit undefined", [1].reduce((_a, b) => b, undefined) === 1);
check("reduceRight order", [1, 2, 3].reduceRight((a, b) => String(a) + b) === "321");
throwsTypeError("reduce all holes without initial value", () => new Array(2).reduce((a, b) => a + b));

// An abrupt callback completion escapes unchanged and prevents later visits.
const marker = {};
let callbacksAfterThrow = 0;
let caughtMarker;
try {
  [1, 2, 3].reduce((accumulator) => {
    callbacksAfterThrow += 1;
    throw marker;
  }, 0);
} catch (error) {
  caughtMarker = error;
}
check("reduce preserves thrown value", caughtMarker === marker);
check("reduce stops after abrupt completion", callbacksAfterThrow === 1);
