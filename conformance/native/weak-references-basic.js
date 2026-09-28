const target = { answer: 42 };
const ref = new WeakRef(target);
console.log(ref.deref() === target);
const registry = new FinalizationRegistry(() => {});
const token = {};
registry.register(target, 'held', token);
console.log(registry.unregister(token), registry.unregister(token));
