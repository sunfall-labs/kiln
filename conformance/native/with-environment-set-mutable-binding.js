// Spec-authority probe for ObjectEnvironmentRecord.SetMutableBinding.
//
// Node v26.8.1 re-resolves the name through the outer `with` environment when the inner getter
// deletes its binding. ECMA-262 instead applies SetMutableBinding to the Object Environment
// Record that produced the original Reference; that record performs HasProperty and Set on its
// own binding object. Keep this probe outside the Node-parity corpus so that the Node divergence
// remains visible in the ordinary update-operator smoke.
const outer = { value: 100 };
const inner = {};
Object.defineProperty(inner, "value", {
  configurable: true,
  get() {
    delete inner.value;
    outer.value = 200;
    return { valueOf() { return 5; } };
  },
});

const update = new Function(
  "outer",
  "inner",
  "with (outer) { with (inner) { return value++; } }",
);
const result = update(outer, inner);
if (result !== 5 || inner.value !== 6 || outer.value !== 200) {
  throw new Error(`wrong SetMutableBinding target: ${result} ${inner.value} ${outer.value}`);
}
console.log(result, inner.value, outer.value);
