class SSG { constructor(self) { this.self = self; this.called = false }
  next(a) { return this.called ? { value: a, done: true } : (this.called = true, { value: this.self, done: false }) }
  [Symbol.iterator]() { return new SSG(this.self) } }
class Eff { constructor(op) { this.op = op } [Symbol.iterator]() { return new SSG(this) } }
function* p() { const a = yield* new Eff('one'); const b = yield* new Eff('two'); return a + b }
const it = p(); let step = it.next(); const outs = [];
while (!step.done) { outs.push(step.value.op); step = it.next(step.value.op.toUpperCase()) }
console.log(outs.join(','), step.value);
