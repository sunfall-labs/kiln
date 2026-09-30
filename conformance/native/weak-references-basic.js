const target = { answer: 42 };
const ref = new WeakRef(target);
console.log(ref.deref() === target);
const symbolTarget = Symbol('weak target');
const symbolRef = new WeakRef(symbolTarget);
console.log(symbolRef.deref() === symbolTarget);
// KeepDuringJob protects the target through every allocation in this synchronous job.
for (let i = 0; i < 16; i++) ({ value: i });
console.log(ref.deref() === target);
const registry = new FinalizationRegistry(() => {});
const token = {};
const held = { answer: 'held' };
registry.register(target, held, token);
console.log(registry.unregister(token), registry.unregister(token));
for (const invalid of [0, null, Symbol.for('registered')]) {
  try { new WeakRef(invalid); console.log('accepted invalid target'); }
  catch (error) { console.log(error instanceof TypeError); }
}
