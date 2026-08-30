// A generator suspends in the middle of its body and resumes there later. An automatic C frame
// cannot be suspended -- it is gone the moment the function returns -- so a generator's locals
// and operand stack live in a heap frame, and re-entry is a switch that jumps back to the label
// after the `yield`. That works because the emitted body is already a flat sequence of labels
// with `goto` between them, which is the shape a bytecode interpreter has.
function* counter(): Generator<number> {
  let n = 0;
  while (n < 3) { yield n; n += 1; }
  return 99;
}
const g = counter();
console.log(g.next(), g.next(), g.next(), g.next(), g.next());

// An infinite generator is fine: nothing runs until it is asked.
function* fib(): Generator<number> {
  let a = 0, b = 1;
  while (true) { yield a; const next = a + b; a = b; b = next; }
}
const f = fib();
console.log(f.next().value, f.next().value, f.next().value, f.next().value, f.next().value);

// What `next(v)` sends becomes the value of the `yield` that suspended.
function* echo(): Generator<number, string, number> {
  const first = yield 1;
  const second = yield first * 2;
  return `got ${first} and ${second}`;
}
const e = echo();
console.log(e.next(), e.next(10), e.next(20), e.next());

// `for...of` and spread both drive the same protocol.
function* range(n: number): Generator<number> { for (let i = 0; i < n; i += 1) { yield i; } }
let walked = "";
for (const x of range(4)) { walked = `${walked}${x},`; }
console.log(walked, [...range(3)], [...range(0)]);

// A generator is its own iterator, which is what lets it be walked at all -- and what makes
// walking a half-consumed one continue rather than restart.
const partial = range(4);
partial.next();
console.log([...partial]);

// `.return(v)` finishes it without resuming the body.
const closed = counter();
console.log(closed.next(), closed.return(42), closed.next());

// State that outlives a suspension: locals, a captured binding, and the operand stack in the
// middle of an expression.
let captured = 100;
function* overCapture(): Generator<number> { yield captured; captured += 1; yield captured; }
const o = overCapture();
console.log(o.next().value, o.next().value, captured);
function* midExpression(): Generator<number> { const total = 1 + (yield 10) + 100; yield total; }
const m = midExpression();
console.log(m.next().value, m.next(5).value);

// Two live at once, each with its own frame.
const first = range(3);
const second = range(3);
first.next();
console.log(first.next().value, second.next().value);

// Spreading a string and an array go through the same instruction, and a string iterates as its
// characters rather than its code units.
console.log([..."abc"], [...[1, 2]], [..."a\u{1F600}b"].length);

// A `try` inside a generator has to still be catching after the `yield` inside it returns, which
// means the handler stack is part of the state a suspension survives -- so it lives in the frame
// beside the locals and the operand stack rather than being automatic.
function* guarded(): Generator<string> {
  try {
    yield "one";
    throw new Error("inside");
  } catch (e: any) {
    yield `caught ${e.message}`;
  } finally {
    yield "finally";
  }
}
const g2 = guarded();
console.log(g2.next(), g2.next(), g2.next(), g2.next());

// The same in an async function, where the suspension is an `await`.
async function safely(): Promise<string> {
  try {
    await Promise.reject(new Error("rejected"));
    return "unreachable";
  } catch (e: any) {
    return `handled ${e.message}`;
  }
}
safely().then((v: string) => console.log(v));

// A handler registered before a suspension and used after one, with a nested generator between.
function* outerGuard(): Generator<string> {
  try {
    for (const value of range(2)) { yield `saw ${value}`; }
    throw "late";
  } catch (e: any) { yield `outer ${e}`; }
}
console.log([...outerGuard()]);

// The frame also retains the receiver across every suspension. Building a generator from a
// method must not replace its `this` with the receiver of a later `.next()` call.
const generatorOwner = {
  tag: "generator owner",
  *read(): Generator<string> {
    yield this.tag;
    return this.tag;
  },
};
const receiverGenerator = generatorOwner.read();
console.log(receiverGenerator.next().value, receiverGenerator.next().value);
