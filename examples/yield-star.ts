// `yield*` is compiled as a loop over the iterator protocol, not as one instruction. That is
// not an optimisation -- it is the only way to get the semantics. Each value has to be yielded
// *and suspended on*, and whatever the caller sends must reach the delegate's own `next`.
function* inner() {
  yield 1;
  yield 2;
  return "inner finished";
}

function* outer() {
  const result = yield* inner();
  yield result;
}
console.log("values then the delegate's return", [...outer()].join(","));

// Two-way: a value sent into the outer generator is handed to the inner one's `next`.
function* echo() {
  const received = yield "asking";
  return "echoed:" + received;
}
function* wrapper() {
  const answer = yield* echo();
  yield answer;
}
const conversation = wrapper();
console.log("asked", conversation.next().value);
console.log("answered", conversation.next("reply").value);

// Any object implementing the protocol works, not only generators -- which is what makes
// `yield*` useful for delegating to a hand-written iterator.
class Once {
  constructor(value) {
    this.value = value;
    this.used = false;
  }
  next(sent) {
    return this.used
      ? { value: sent, done: true }
      : ((this.used = true), { value: this.value, done: false });
  }
  [Symbol.iterator]() {
    return new Once(this.value);
  }
}
function* delegating() {
  const sent = yield* new Once("from a plain object");
  return sent;
}
const custom = delegating();
console.log("custom protocol", custom.next().value);
console.log("its return value", custom.next("sent back").value);

console.log("string methods", "abc".toUpperCase(), "A,B".split(",").join("|"), " x ".trim());
