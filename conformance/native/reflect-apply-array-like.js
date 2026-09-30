function join(a, b) {
  return this.prefix + a + b;
}

const args = { 0: "A", 1: "B", length: "2" };
console.log(Reflect.apply(join, { prefix: "!" }, args));
console.log(Reflect.apply(function () { return arguments.length; }, null, { length: 0 }));
