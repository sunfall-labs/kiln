// Computed property definitions exercise the single headed descriptor walk.
// Node is the oracle: report UTF-16 units so lone surrogates stay distinct from U+FFFD.
function units(text) {
  const result = [];
  for (let index = 0; index < text.length; index += 1) result.push(text.charCodeAt(index));
  return result;
}
function same(actual, expected, message) {
  if (JSON.stringify(actual) !== JSON.stringify(expected)) {
    throw new Error(message + ": " + JSON.stringify(actual) + " != " + JSON.stringify(expected));
  }
}

const high = String.fromCharCode(0xd800);
const low = String.fromCharCode(0xdc00);
const replacement = "\ufffd";
const object = {};
Object.defineProperty(object, high, {
  get() { return 17; }, enumerable: true, configurable: true,
});
Object.defineProperty(object, replacement, { value: 23, enumerable: true, configurable: true });
same(units(Object.keys(object)[0]), [0xd800], "high key");
same(units(Object.keys(object)[1]), [0xfffd], "replacement key");
same([object[high], object[replacement]], [17, 23], "getter values");
same(units(Object.getOwnPropertyDescriptor(object, high).get.name), [103, 101, 116], "getter reflection");

const methods = {
  [high]() { return 41; },
  [replacement]() { return 43; },
};
same(units(methods[high].name), [0xd800], "computed method high name");
same(units(methods[replacement].name), [0xfffd], "computed method replacement name");
const accessors = {
  get [high]() { return 47; },
  set [replacement](value) { this._replacement = value; },
};
same(units(Object.getOwnPropertyDescriptor(accessors, high).get.name),
     [103, 101, 116, 32, 0xd800], "computed getter high name");
same(units(Object.getOwnPropertyDescriptor(accessors, replacement).set.name),
     [115, 101, 116, 32, 0xfffd], "computed setter replacement name");

const calls = [];
const proxy = new Proxy({}, {
  defineProperty(target, key, descriptor) {
    calls.push(units(key));
    return Reflect.defineProperty(target, key, descriptor);
  },
  getOwnPropertyDescriptor(target, key) {
    calls.push(units(key));
    return Reflect.getOwnPropertyDescriptor(target, key);
  },
});
Object.defineProperty(proxy, low, { value: 31, writable: true, enumerable: true, configurable: true });
same(calls[0], [0xdc00], "proxy define key");
same(units(Object.keys(proxy)[0]), [0xdc00], "proxy stored key");
same([proxy[low], Object.prototype.hasOwnProperty.call(proxy, low)], [31, true], "proxy value");
console.log("definitions", JSON.stringify({
  keys: Object.keys(object).map(units), values: [object[high], object[replacement]],
  proxyKey: units(Object.keys(proxy)[0]), proxyValue: proxy[low], calls,
}));
