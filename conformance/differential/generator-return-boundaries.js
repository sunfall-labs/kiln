function* body() {
  yield "body";
}

const payload = { marker: "return" };
const unstarted = body();
const beforeStart = unstarted.return(payload);
console.log("unstarted", beforeStart.value === payload, beforeStart.done);

const completed = body();
completed.next();
completed.next();
const afterCompletion = completed.return(payload);
console.log("completed", afterCompletion.value === payload, afterCompletion.done);

function* caughtThrow(yieldAgain) {
  try {
    yield "body";
  } finally {
    try {
      yield "cleanup";
    } catch (error) {
      if (yieldAgain) yield error;
    }
  }
}

for (const yieldAgain of [false, true]) {
  const iterator = caughtThrow(yieldAgain);
  iterator.next();
  const closing = iterator.return(payload);
  const handled = iterator.throw("handled");
  const result = yieldAgain ? iterator.next("resume") : handled;
  console.log("caught", yieldAgain, closing.value, closing.done,
    handled.value === (yieldAgain ? "handled" : payload), handled.done,
    result.value === payload, result.done);
}

const thrown = { marker: "throw" };
try {
  completed.throw(thrown);
} catch (error) {
  console.log("completed-throw", error === thrown);
}
