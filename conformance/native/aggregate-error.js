// The native backend should expose the standard AggregateError constructor. This first
// assertion is intentionally a direct constructor path, not Promise.any's internal helper.
const error = new AggregateError([new Error("x")], "msg");
console.log(
  error.constructor === AggregateError,
  error instanceof AggregateError,
  error instanceof Error,
  AggregateError.prototype.constructor === AggregateError,
  error.name,
  error.message,
  Array.isArray(error.errors),
  error.errors[0].message,
  Object.keys(error).join(","),
);

const iterable = {
  [Symbol.iterator]: () => {
    let done = false;
    return {
      next: () => {
        if (done) return { done: true };
        done = true;
        return { value: 7, done: false };
      },
    };
  },
};
const called = AggregateError(iterable, "called");
console.log(called.constructor === AggregateError, called.message, called.errors.join(","));
