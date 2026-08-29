// `finally` has to run on every way out of a `try`: falling off the end, `return`, `break`,
// `continue`, and an exception. Each exit emits the cleanup inline, so every one of them stays
// an ordinary jump rather than needing a pending-completion register.

function withReturn(): string {
  try {
    return "returned";
  } finally {
    // Runs before the value reaches the caller, but after it has been computed.
    console.log("cleanup ran before the return completed");
  }
}
console.log(withReturn());

function throwing() {
  try {
    throw new Error("failed");
  } finally {
    console.log("cleanup ran on the way out");
  }
}
try {
  throwing();
} catch (error) {
  console.log("caught", error.message);
}

// A cleanup inside a loop runs on `continue` and on `break` too.
function loop(): string {
  for (let i = 0; i < 3; i++) {
    try {
      if (i === 1) {
        continue;
      }
      if (i === 2) {
        break;
      }
    } finally {
      console.log("iteration cleanup", i);
    }
  }
  return "done";
}
console.log(loop());

// An exception unwinds across calls, restoring each frame as a normal return would.
function inner() {
  throw new Error("from inner");
}
function outer() {
  inner();
}
try {
  outer();
} catch (error) {
  console.log("crossed two frames", error.message);
}

// Any value can be thrown, not only an error.
try {
  throw "a string";
} catch (value) {
  console.log("non-error", value);
}

// `catch` without a binding discards the value.
try {
  throw new Error("ignored");
} catch {
  console.log("caught without binding");
}
