const record = {};
for (let index = 0; index < 40; index += 1) {
  record[`key${index}`] = index;
}

// Exercise identifier resolution over an ordinary object after it has more than the
// implementation's small-shape range. The body is compiled dynamically so this covers the
// same own-property lookup used by captured `with` environments.
const readThroughWith = Function(
  "record",
  "with (record) { return [key37, key4, typeof missingKey]; }",
);
console.log(JSON.stringify(readThroughWith(record)));

delete record.key5;
console.log(JSON.stringify([
  record.key4,
  record.key5,
  readThroughWith(record),
]));
record.key5 = "reinserted";
console.log(JSON.stringify([
  record.key5,
  record["key39"],
  record["missingKey"],
  Object.keys(record).slice(-4),
]));

// Integer-index keys sort before string keys, even when inserted after them. Adding another
// index must not disturb the string insertion order.
record["20"] = "twenty";
record["2"] = "two";
record["100"] = "one hundred";
console.log(JSON.stringify([
  record["2"], record["20"], record["100"], record["999"],
  record.key0, record.key39, Object.keys(record).slice(0, 4),
]));

let getterCalls = 0;
Object.defineProperty(record, "key10", {
  configurable: true,
  enumerable: true,
  get() {
    getterCalls += 1;
    return `getter:${getterCalls}`;
  },
});
console.log(JSON.stringify([
  record["key10"],
  Object.getOwnPropertyDescriptor(record, "key10").get !== undefined,
  Object.keys(record).includes("key10"),
  getterCalls,
]));
Object.defineProperty(record, "key10", {
  configurable: true,
  enumerable: true,
  value: "data-again",
  writable: true,
});
console.log(JSON.stringify([
  record.key10,
  Object.getOwnPropertyDescriptor(record, "key10").get === undefined,
  getterCalls,
]));

// Distinct lone UTF-16 units and U+FFFD are distinct property names.
record["\uD800"] = "high";
record["\uD801"] = "next-high";
record["\uFFFD"] = "replacement";
console.log(JSON.stringify([
  record["\uD800"], record["\uD801"], record["\uFFFD"],
]));

// A symbol and its same-description string are separate own keys, both visible to Reflect,
// while Object.keys reports only the string.
const sameDescription = Symbol("same");
const secondSameDescription = Symbol("same");
record[sameDescription] = "symbol";
record[secondSameDescription] = "second symbol";
record.same = "string";
console.log(JSON.stringify([
  record[sameDescription],
  record[secondSameDescription],
  record.same,
  Reflect.ownKeys(record).filter((key) =>
    key === sameDescription || key === secondSameDescription || key === "same").length,
  Object.keys(record).includes("same"),
]));
