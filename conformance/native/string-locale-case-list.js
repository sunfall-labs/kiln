function result(fn) {
  try {
    return String(fn());
  } catch (error) {
    return error.name;
  }
}

console.log("primitive list:", "I".toLocaleLowerCase(42));
console.log("empty string locale:", result(() => "I".toLocaleLowerCase("")));
console.log("empty array locale:", result(() => "I".toLocaleLowerCase([""])));
console.log("empty list:", "I".toLocaleLowerCase([]));
console.log("hole:", result(() => "I".toLocaleLowerCase({ length: 1 })));
console.log("present undefined:", result(() => "I".toLocaleLowerCase({ length: 1, 0: undefined })));
console.log("numeric element:", result(() => "I".toLocaleLowerCase([42])));
console.log("object element:", "I".toLocaleLowerCase([new String("tr")]));

let laterGetterCalls = 0;
const invalidThenGetter = {
  length: 2,
  0: "not a valid locale",
  get 1() {
    laterGetterCalls++;
    return "en";
  },
};
console.log("invalid order:", result(() => "I".toLocaleLowerCase(invalidThenGetter)), laterGetterCalls);
