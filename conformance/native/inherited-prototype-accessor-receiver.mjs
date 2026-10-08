function check(condition, message) {
  if (!condition) throw new Error(message);
}

const objectReceiver = { marker: "object receiver" };
const explicitReceiver = { marker: "explicit receiver" };
Object.defineProperty(Object.prototype, "receiverProbe", {
  configurable: true,
  get() { return this.marker; },
});
try {
  check(objectReceiver.receiverProbe === "object receiver",
    "an inherited builtin-prototype getter receives the object that started [[Get]]");
  check(Reflect.get(Object.prototype, "receiverProbe", explicitReceiver) === "explicit receiver",
    "Reflect.get keeps its explicit receiver while reading a builtin prototype store");
} finally {
  delete Object.prototype.receiverProbe;
}

console.log("inherited prototype accessor receiver passes");
