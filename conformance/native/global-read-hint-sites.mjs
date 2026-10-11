// Repeated reads of a global name from one site must observe every later change to the global
// object: reassignment, accessor replacement, deletion of this name or of an earlier-defined
// one (which moves the slot a site remembered), shadowing, and reads across calls.
const log = (...parts) => console.log(parts.join(" "));

function sqrtOf(n) {
  return Math.sqrt(n);
}
function mathIsOriginal(original) {
  return Math === original;
}
function readMathName() {
  try {
    return typeof Math;
  } catch (error) {
    return "threw " + error.name;
  }
}
function readMathStrict() {
  try {
    return Math.sqrt(16);
  } catch (error) {
    return "threw " + error.name;
  }
}

const original = Math;

// Warm the sites on the intrinsic.
let total = 0;
for (let i = 0; i < 5; i++) total += sqrtOf(i * i);
log("warm", total, mathIsOriginal(original), readMathName());

// Reassignment is seen by the very next read.
globalThis.Math = { sqrt: (n) => "fake" + n };
log("reassigned", sqrtOf(4), mathIsOriginal(original), readMathName());
globalThis.Math = original;
log("restored", sqrtOf(4), mathIsOriginal(original));

// Accessor in place of the data property, with a getter that counts reads.
let reads = 0;
Object.defineProperty(globalThis, "Math", {
  configurable: true,
  get() {
    reads++;
    return original;
  },
});
log("accessor", sqrtOf(9), sqrtOf(25), reads > 0);
Object.defineProperty(globalThis, "Math", {
  configurable: true,
  writable: true,
  enumerable: false,
  value: original,
});
log("data again", sqrtOf(36), mathIsOriginal(original));

// Deleting an earlier-defined global shifts later slots; a remembered slot must not be trusted.
const savedString = globalThis.String;
const savedNumber = globalThis.Number;
delete globalThis.String;
delete globalThis.Number;
log("after deleting earlier", sqrtOf(49), mathIsOriginal(original), typeof savedString);
globalThis.String = savedString;
globalThis.Number = savedNumber;

// Deleting the name itself.
delete globalThis.Math;
// `typeof` of the deleted name is left to docs/issues/typeof-deleted-builtin-global.md.
log("deleted", "Math" in globalThis, readMathStrict());
globalThis.Math = original;
log("back", readMathName(), readMathStrict());

// Writable false: the value is still read, and a write (strict: a module) throws and changes nothing.
Object.defineProperty(globalThis, "Math", { writable: false, value: original });
try { globalThis.Math = { sqrt: () => "nope" }; } catch (e) { log("readonly write", e.constructor.name); }
log("readonly", sqrtOf(64), mathIsOriginal(original));
Object.defineProperty(globalThis, "Math", { writable: true, value: original });

// Shadowing: a parameter, a local, a block binding and a class member are not the global.
function withParameter(Math) {
  return Math.sqrt(2);
}
function withLocal() {
  const Math = { sqrt: () => "local" };
  return Math.sqrt(2);
}
function withBlock() {
  let result = sqrtOf(81);
  {
    const Math = { sqrt: () => "block" };
    result += "/" + Math.sqrt(1);
  }
  return result + "/" + sqrtOf(100);
}
log("shadow", withParameter({ sqrt: () => "param" }), withLocal(), withBlock());

// Several globals read through the same function, interleaved with calls that change them.
function trio() {
  return [typeof Math, typeof JSON, typeof Array, typeof Reflect].join(",");
}
log("trio", trio());
const savedJSON = globalThis.JSON;
globalThis.JSON = 1;
log("trio json=1", trio());
delete globalThis.JSON;
log("trio json gone", "JSON" in globalThis);
globalThis.JSON = savedJSON;
log("trio restored", trio());

// A new global defined after warm-up is found, and an unbound one still throws.
globalThis.freshGlobalForHints = 7;
function readFresh() {
  try {
    return freshGlobalForHints;
  } catch (error) {
    return "threw " + error.name;
  }
}
log("fresh", readFresh());
delete globalThis.freshGlobalForHints;
log("fresh gone", readFresh());
