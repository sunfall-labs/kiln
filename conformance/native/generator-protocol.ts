// Calling a generator function does not run it, and that split is the whole of its object model:
// the *parameters* bind at the call, the body runs at the first `next`, and everything after that
// is the consumer driving a suspended frame. Each half is observable, so each is asked about here.

// Parameters bind at [[Call]]. A default with a side effect proves it: the effect happens when the
// generator is *made*, and the body has still not started.
let evaluated = 0;
function sideEffect(): number { evaluated += 1; return evaluated; }
function* withDefault(seed: number = sideEffect()): Generator<number> {
  console.log("body started");
  yield seed;
  yield seed + 1;
}
const counting = withDefault();
console.log("made, effects so far:", evaluated);
console.log(counting.next(), counting.next(), counting.next(), counting.next());

// An argument that is passed suppresses the default, and is bound at the call rather than read
// later -- a generator holds the values it was given, not the expressions.
let source = 10;
function* held(value: number = 0): Generator<number> { yield value; }
const captured = held(source);
source = 999;
console.log(captured.next(), evaluated);

// `next(v)` sends a value *in*: it becomes the result of the `yield` that suspended, so a
// generator is a two-way channel rather than a sequence. The first `next` has nowhere to send to.
function* echo(): Generator<string, string, string> {
  const first = yield "ask 1";
  const second = yield `got ${first}`;
  return `done ${second}`;
}
const conversation = echo();
console.log(conversation.next("ignored"), conversation.next("a"), conversation.next("b"), conversation.next("c"));

// The generator is its own iterable, which is what lets one be spread or walked directly.
function* three(): Generator<number> { yield 1; yield 2; yield 3; }
const walk = three();
console.log(walk[Symbol.iterator]() === walk, [...three()], Array.from(three()));
for (const value of three()) { console.log("saw", value); }

// A completed generator stays completed: every later `next` is the same finished step, and the
// body is never re-entered.
const spent = three();
console.log([...spent], spent.next(), spent.next());

// `return(v)` finishes it without resuming the body, so the value is the caller's and the body's
// remaining yields never happen. Before the first `next` there was nothing to finish either.
const abandoned = three();
console.log(abandoned.next(), abandoned.return(42), abandoned.next());
const untouched = three();
console.log(untouched.return("early"), untouched.next());
// What is deliberately *not* asked here: `return()` on a generator suspended inside a `try`. The
// language resumes the body in return completion so a `finally` runs; this backend cannot re-enter
// a body in that mode and refuses rather than skipping the cleanup silently. See NOTES.md.

// A generator resuming *itself* is a TypeError rather than a fresh frame: there is one body and it
// is already running, which is the same reason a re-entrant `next` from inside the body fails.
let reentrant: any;
function* selfish(): Generator<number> {
  yield 1;
  reentrant.next();
  yield 2;
}
reentrant = selfish();
console.log(reentrant.next());
try { console.log(reentrant.next()); } catch (error: any) { console.log(error.name, error.message); }

// `yield*` delegates: the inner generator's steps become the outer one's, and its *return* value
// becomes the value of the `yield*` expression rather than a step.
function* inner(): Generator<number, string> { yield 1; yield 2; return "inner done"; }
function* outer(): Generator<number | string> {
  const result = yield* inner();
  yield result;
}
console.log([...outer()]);

// The value sent into a delegated yield can have a different type from the yielded value, and
// the delegate's completion value can be an object. Both values cross synthetic resume edges.
function* sentInner(): Generator<number, { received: string }, string> {
  const received = yield 10;
  return { received };
}
function* sentOuter(): Generator<number, { received: string }, string> {
  return yield* sentInner();
}
const sentGenerator = sentOuter();
console.log("delegate sent", sentGenerator.next(), sentGenerator.next("text"));

// throw() enters the delegate and its return value becomes the outer yield* result.
function* catchingInner(): Generator<number, string, never> {
  try {
    yield 20;
  } catch (error) {
    return `caught ${String(error)}`;
  }
}
function* catchingOuter(): Generator<number, string> {
  return yield* catchingInner();
}
const thrown = catchingOuter();
console.log("delegate throw", thrown.next(), thrown.throw("boom"));

// A generator method on an object, and one on a class, are the same thing reached two ways.
const collection = { *[Symbol.iterator]() { yield "a"; yield "b"; } };
class Range {
  limit: number;
  constructor(limit: number) { this.limit = limit; }
  *values(): Generator<number> { for (let n = 0; n < this.limit; n += 1) { yield n; } }
}
console.log([...collection], [...new Range(3).values()]);

// Local state survives suspension, which is the point of the whole machinery: the frame is kept,
// not rebuilt.
function* accumulating(): Generator<number> {
  let total = 0;
  for (let n = 1; n <= 3; n += 1) { total += n; yield total; }
}
console.log([...accumulating()]);
