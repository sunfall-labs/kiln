// A prepared member call evaluates its property before evaluating callback arguments.  The
// parallel lowering may bypass the installed wrapper only when that saved value is still the
// builtin extension; otherwise it must invoke the saved value (including an inherited getter).
const original = Array.prototype.parallelMap;
const values = [];
for (let index = 0; index < 4096; index++) values.push(index);
const mapped = values.parallelMap((value) => value * 2 + 1);
let calls = 0;
let mutatedResult;
try {
  mutatedResult = [1, 2, 3].parallelMap((value) => {
    calls++;
    if (value === 1) {
      Array.prototype.parallelMap = function () {
        return ["replacement"];
      };
    }
    return value + 1;
  });
} finally {
  Array.prototype.parallelMap = original;
}

const inherited = Object.create(Array.prototype);
let getterCalls = 0;
Object.defineProperty(inherited, "parallelMap", {
  get() {
    getterCalls++;
    return function (callback) {
      return callback(this[0]);
    };
  },
});
const receiver = [4];
Object.setPrototypeOf(receiver, inherited);
const inheritedResult = receiver.parallelMap((value) => value + 2);

const forwarding = Object.create(Array.prototype);
let forwardingGetterCalls = 0;
Object.defineProperty(forwarding, "parallelMap", {
  get() {
    forwardingGetterCalls++;
    return original;
  },
});
const forwardingReceiver = [6, 7];
Object.setPrototypeOf(forwardingReceiver, forwarding);
const forwardingResult = forwardingReceiver.parallelMap((value) => value + 3);

console.log(JSON.stringify({
  mapped: [mapped.length, mapped[0], mapped[mapped.length - 1]],
  mutatedResult,
  calls,
  getterCalls,
  inheritedResult,
  forwardingGetterCalls,
  forwardingResult,
}));
