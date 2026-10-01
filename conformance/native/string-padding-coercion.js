// Padding converts targetLength first and skips filler conversion when the receiver is long
// enough. These are order-sensitive user-code checks, not just output examples.
const events = [];
const receiver = "already-long";
const unusedFiller = {
  toString() {
    events.push("unused filler");
    throw new Error("the filler should not be observed");
  },
};

if (receiver.padStart(0, unusedFiller) !== receiver || receiver.padEnd(-1, unusedFiller) !== receiver ||
    receiver.padStart(NaN, unusedFiller) !== receiver || "abc".padEnd(2, Symbol("unused")) !== "abc" ||
    events.length !== 0) {
  throw new Error("padding converted a filler after its target length was already satisfied");
}

const order = [];
const padded = "x".padStart({
  valueOf() {
    order.push("target.valueOf");
    return {};
  },
  toString() {
    order.push("target.toString");
    return "4.8";
  },
}, {
  toString() {
    order.push("filler.toString");
    return "ab";
  },
});
if (padded !== "abax" || order.join(",") !== "target.valueOf,target.toString,filler.toString") {
  throw new Error(`padding conversion order/value mismatch: ${padded}; ${order.join(",")}`);
}

const concatOrder = [];
const lone = "\uD800";
const joined = ("ab".slice(1)).concat(
  { toString() { concatOrder.push("first"); return lone; } },
  { toString() { concatOrder.push("second"); return "z"; } },
);
if (joined.length !== 3 || joined.charCodeAt(0) !== 98 || joined.charCodeAt(1) !== 0xd800 ||
    joined.charCodeAt(2) !== 122 || concatOrder.join(",") !== "first,second") {
  throw new Error("multi-argument concat lost conversion order or UTF-16 units");
}

let infiniteLength = false;
let hugeLength = false;
let symbolTarget = false;
try {
  "x".padStart(Infinity);
} catch (error) {
  infiniteLength = error instanceof RangeError;
}
try {
  "x".padEnd(1e20);
} catch (error) {
  hugeLength = error instanceof RangeError;
}
try {
  "x".padStart(Symbol("length"));
} catch (error) {
  symbolTarget = error instanceof TypeError;
}
if ("x".padEnd(2.9, "-") !== "x-" || "x".padStart(-Infinity, "-") !== "x" ||
    !infiniteLength || !hugeLength || !symbolTarget) {
  throw new Error("padding ToLength normalization mismatch");
}
