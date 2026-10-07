function subject(calls) {
  const array = ["zero", "one", "two"];
  Object.defineProperty(array, "0", {
    configurable: true,
    enumerable: true,
    get() {
      calls.push("get0");
      Object.defineProperty(array, "1", { enumerable: true });
      delete array[2];
      return "zero";
    },
  });
  Object.defineProperty(array, "1", {
    configurable: true,
    enumerable: false,
    value: "one",
  });
  Object.defineProperty(array, "2", {
    configurable: true,
    enumerable: true,
    value: "two",
  });
  return array;
}

const keyCalls = [];
const keyArray = subject(keyCalls);
console.log(JSON.stringify([Object.keys(keyArray), keyCalls]));

const valueCalls = [];
console.log(JSON.stringify([Object.values(subject(valueCalls)), valueCalls]));

const entryCalls = [];
console.log(JSON.stringify([Object.entries(subject(entryCalls)), entryCalls]));

const plain = ["a", , "c"];
console.log(JSON.stringify([Object.keys(plain), Object.values(plain), Object.entries(plain)]));
