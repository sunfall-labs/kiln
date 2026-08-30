// `for await` over a plain array of promises: the iterator is sync, so each *value* is awaited.
async function overPromises() {
  const out = [];
  for await (const value of [Promise.resolve(1), 2, Promise.resolve(3)]) {
    out.push(value);
  }
  return out.join(",");
}

// An async iterator hands back a promise of the whole result object instead.
function counter(limit) {
  let n = 0;
  return {
    [Symbol.asyncIterator]() {
      return {
        next() {
          n += 1;
          return Promise.resolve(n > limit ? { done: true } : { value: n, done: false });
        },
      };
    },
  };
}

async function overAsyncIterator() {
  const out = [];
  for await (const value of counter(3)) {
    out.push(value);
  }
  return out.join("-");
}

// `break` still closes the loop, and the body's bindings are per-iteration.
async function stopsEarly() {
  const seen = [];
  for await (const value of [1, 2, 3, 4]) {
    if (value === 3) break;
    seen.push(() => value);
  }
  return seen.map((read) => read()).join("");
}

// Destructuring in the head works the same as in a plain `for-of`.
async function destructures() {
  const out = [];
  for await (const [key, value] of [Promise.resolve(["a", 1]), ["b", 2]]) {
    out.push(key + value);
  }
  return out.join(",");
}

// A rejection inside the loop propagates out of the async function.
async function propagates() {
  try {
    for await (const value of [Promise.reject(new Error("boom"))]) {
      return "unreachable" + value;
    }
  } catch (error) {
    return "caught " + error.message;
  }
  return "none";
}

async function main() {
  console.log(await overPromises());
  console.log(await overAsyncIterator());
  console.log(await stopsEarly());
  console.log(await destructures());
  console.log(await propagates());
}
main();
