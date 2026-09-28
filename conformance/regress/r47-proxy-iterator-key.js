const seen = [];
const source = new Proxy({}, {
  get(_target, key) {
    if (key === Symbol.iterator) {
      seen.push("symbol");
      return function* () {
        yield 1;
        yield 2;
      };
    }
    seen.push(typeof key);
    return undefined;
  },
});

console.log([...source].join(","), seen.join(","));
