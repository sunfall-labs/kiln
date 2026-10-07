// Date's math and numeric conversions must use the primordials captured at bootstrap, not later
// replacements of global Math/Number properties.
const getterNames = [
  "getUTCFullYear", "getUTCMonth", "getUTCDate", "getUTCDay", "getUTCHours",
  "getUTCMinutes", "getUTCSeconds", "getUTCMilliseconds", "getYear",
];
const date = new Date(-1234567890123);
const before = getterNames.map((name) => date[name]()).join(",");
const invalid = new Date(NaN);
const invalidOffsetWasNaN = invalid.getTimezoneOffset() !== invalid.getTimezoneOffset();

const setterExpected = new Date(-1);
setterExpected.setUTCHours(3, 4, 5, 6);
const expectedSetterTime = setterExpected.getTime();
const setterActual = new Date(-1);
const invalidSetter = new Date(NaN);
const conversionOrder = [];
const conversion = (name, value) => ({
  valueOf() {
    conversionOrder.push(name);
    return value;
  },
});

const originalNumber = Number;
const fail = () => { throw new Error("Date observed a replaced numeric intrinsic"); };
Math.floor = fail;
Math.trunc = fail;
Math.abs = fail;
originalNumber.isFinite = fail;
originalNumber.isNaN = fail;
globalThis.Number = fail;

const after = getterNames.map((name) => date[name]()).join(",");
setterActual.setUTCHours(
  conversion("hour", 3),
  conversion("minute", 4),
  conversion("second", 5),
  conversion("millisecond", 6),
);
const invalidSetterResult = invalidSetter.setUTCDate(conversion("invalid-date", 1));
const brandCheck = (() => {
  try {
    Date.prototype.getUTCHours.call({});
    return false;
  } catch (error) {
    return error instanceof TypeError;
  }
})();

console.log(
  before === after,
  setterActual.getTime() === expectedSetterTime,
  conversionOrder.join(","),
  invalidSetterResult !== invalidSetterResult,
  invalidOffsetWasNaN,
  brandCheck,
);
