// Reflect.get follows the same indexed-property path as bracket access.
const array = [1, 2];
const typed = new Uint8Array([7, 8]);
console.log(Reflect.get(array, "0"), Reflect.get(array, 1));
console.log(Reflect.get(typed, "0"), Reflect.get(typed, 1));

// A forwarding proxy is the pattern used by copy-on-write libraries such as Immer.
const forwarded = new Proxy(array, {
  get(target, key) { return Reflect.get(target, key); },
});
forwarded.push(3);
console.log(array.join(","));

// The optional receiver is observable both by accessors and by a proxy's get trap.
const getterTarget = { get value() { return this.answer; } };
console.log(Reflect.get(getterTarget, "value", { answer: 42 }));
const receiverTarget = new Proxy({}, {
  get(target, key, receiver) { return receiver.answer; },
});
console.log(Reflect.get(receiverTarget, "value", { answer: 43 }));

// Unlike ordinary property access, Reflect.get does not box a primitive target.
try { Reflect.get(1, "value"); } catch (error) {
  console.log("reflect target", error instanceof TypeError);
}
