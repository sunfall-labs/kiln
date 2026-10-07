// Canonical array-index names use integer-key ordering and array-element storage. Leading-zero
// and 2^32-1 spellings remain ordinary names; 2^32-2 is still an index on ordinary objects.
const object = {};
object["4294967295"] = "after-max";
object["01"] = "leading-zero";
object["4294967294"] = "max-index";
object["0"] = "zero";
console.log(Object.keys(object).join(","));

const array = ["zero", "one"];
const symbol = Symbol("1");
array["01"] = "named";
array["4294967295"] = "after-max";
array[symbol] = "symbol";
console.log(array[1], array[symbol], array.length);
console.log(Object.keys(array).join(","));
console.log(Object.getOwnPropertySymbols(array).length);
