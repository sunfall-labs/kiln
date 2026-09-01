// `Promise.prototype.catch(r)` is `Invoke(this, "then", [undefined, r])` -- literally, including
// the read. Both engines called the intrinsic `then` directly instead, so a thenable that is not
// a promise was refused outright: `Promise.prototype.catch.call(thenable, f)` answered "a object
// is not a function" about a receiver whose `then` was right there.
//
// Only `catch` is here. `then` needs a promise to attach a reaction to and `finally` needs a pair
// of closures over the receiver that neither engine can build for a thenable yet, so both still
// refuse a non-promise receiver -- and refuse it the same way.
let handlers;
const thenable = {
  then(onFulfilled, onRejected) {
    handlers = [typeof onFulfilled, typeof onRejected];
    return "from then";
  },
};
// The value `then` returns is `catch`'s own result: nothing is wrapped on the way back.
console.log("delegates", Promise.prototype.catch.call(thenable, () => {}), handlers.join(","));

// A receiver whose `then` is not callable is a TypeError from `catch`, not a silent nothing.
try { Promise.prototype.catch.call({ then: 5 }, () => {}); console.log("not-callable", "no throw"); }
catch (error) { console.log("not-callable", error.constructor.name); }

// A promise receiver still takes the ordinary path, and `catch` still chains.
Promise.resolve(1).catch(() => "unused").then((value) => console.log("chained", value));
