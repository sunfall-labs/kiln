// `call`, `apply` and `bind` -- the three ways a program supplies a receiver by hand.
function describe(prefix, suffix) {
  return prefix + this.name + suffix;
}

const subject = { name: "kiln" };
console.log(describe.call(subject, "<", ">"));
console.log(describe.apply(subject, ["[", "]"]));

// `bind` fixes the receiver and any number of leading arguments.
const bound = describe.bind(subject, "{");
console.log(bound("}"));
// A bound function reports what is still missing, and says whose it is.
console.log(typeof bound, bound.length, bound.name);

// Binding a bound function fixes more arguments and keeps the first receiver.
const twice = bound.bind({ name: "ignored" });
console.log(twice("!"));

// `apply` with a nullish list means no arguments at all.
console.log(describe.apply(subject, undefined));

// A native is callable through them too.
console.log(Math.max.apply(null, [3, 9, 4]));
console.log(Array.prototype.join.call([1, 2, 3], "-"));

// The receiver is forwarded, not the wrapper: a method extracted and rebound still sees it.
class Counter {
  constructor() {
    this.total = 0;
  }
  add(step) {
    this.total += step;
    return this.total;
  }
}
const counter = new Counter();
const add = counter.add.bind(counter);
add(2);
console.log(add(3));
