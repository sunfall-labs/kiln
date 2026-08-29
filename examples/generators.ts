// A generator is the first thing in this VM whose frame does not run to completion. Calling one
// does not run the body: it captures the arguments and hands back an object whose `next` runs
// the body up to each `yield`, then suspends -- registers, operand stack and all.
function* counter() {
  yield 1;
  yield 2;
  yield 3;
}

const manual = counter();
console.log("manual", manual.next().value, manual.next().value, manual.next().value);
console.log("exhausted", manual.next().done, manual.next().value);

// A generator object is its own iterator, so it works anywhere an iterable does.
const collected: number[] = [];
for (const value of counter()) {
  collected.push(value);
}
console.log("for-of", collected.join(","));
console.log("spread", [...counter()].join(","));

// `yield` is an expression: it evaluates to whatever the caller sends with the next `next(v)`.
// That is the whole reason a generator can receive values as well as produce them.
function* echo() {
  const first = yield "ready";
  const second = yield first * 2;
  return second + 1;
}
const conversation = echo();
console.log("sent", conversation.next().value);
console.log("doubled", conversation.next(21).value);
console.log("returned", conversation.next(99).value);

// A generator suspended part-way through an expression resumes into the middle of it.
function* partial() {
  const total = 1 + (yield "pause") + 100;
  return total;
}
const resumed = partial();
resumed.next();
console.log("resumed mid-expression", resumed.next(10).value);
