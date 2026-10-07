// Array map captures its starting length, but rechecks HasProperty/Get at every index.
const map = Array.prototype.map;

const values = [1, 2];
const grown = values.parallelMap(function (value, index) {
  if (index === 0) {
    values.length = 0;
    values.push(8, 2);
  }
  return value;
});
if (grown.length !== 2 || grown[0] !== 1 || grown[1] !== 2) {
  throw new Error("map did not use its captured length after receiver growth");
}

const prior = Object.getOwnPropertyDescriptor(Array.prototype, "1");
const sparse = [3, 4];
let inherited;
try {
  inherited = Reflect.apply(map, sparse, [function (value, index) {
    if (index === 0) {
      sparse.length = 0;
      Object.defineProperty(Array.prototype, "1", { value: 11, configurable: true });
      sparse.length = 2;
    }
    return value;
  }]);
} finally {
  if (prior) Object.defineProperty(Array.prototype, "1", prior);
  else delete Array.prototype["1"];
}
if (inherited.length !== 2 || inherited[0] !== 3 || inherited[1] !== 11) {
  throw new Error("map failed to observe an inherited index added during iteration");
}

console.log("array-map-initial-length", grown.length, grown[0], grown[1], inherited[1]);
