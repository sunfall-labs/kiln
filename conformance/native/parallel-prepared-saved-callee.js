// A prepared member call evaluates its property before evaluating callback arguments.  The
// parallel lowering may bypass the installed wrapper only when that saved value is still the
// builtin extension; otherwise it must invoke the saved value (including an inherited getter).
const original = Array.prototype.parallelMap;
const values = [];
for (let index = 0; index < 4096; index++) values.push(index);
const mapped = values.parallelMap((value) => value * 2 + 1);
let calls = 0;
let mutatedResult;
let argumentEffects = 0;
let argumentReplacementCalls = 0;
let callbackReplacementCalls = 0;
let argumentMutationResult;
try {
  // Mutate the method after its property has been saved but before the pure callback is
  // supplied. The pool path must honor the saved callee and evaluate this effect once.
  argumentMutationResult = values.parallelMap(
    (argumentEffects++,
      (Array.prototype.parallelMap = function () {
        argumentReplacementCalls++;
        return ["replacement"];
      }),
      (value) => value + 3),
  );
  if (
    argumentEffects !== 1 ||
    argumentReplacementCalls !== 0 ||
    argumentMutationResult.length !== 4096 ||
    argumentMutationResult[0] !== 3 ||
    argumentMutationResult[4095] !== 4098
  ) {
    throw new Error("prepared call did not preserve its saved callee across argument evaluation");
  }
  Array.prototype.parallelMap = original;
  mutatedResult = [1, 2, 3].parallelMap((value) => {
    calls++;
    if (value === 1) {
      Array.prototype.parallelMap = function () {
        callbackReplacementCalls++;
        return ["replacement"];
      };
    }
    return value + 1;
  });
  if (
    calls !== 3 ||
    callbackReplacementCalls !== 0 ||
    JSON.stringify(mutatedResult) !== JSON.stringify([2, 3, 4])
  ) {
    throw new Error("active method call did not keep using its original callee");
  }
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
  argumentMutationResult: [argumentMutationResult.length, argumentMutationResult[0], argumentMutationResult[4095]],
  argumentEffects,
  argumentReplacementCalls,
  callbackReplacementCalls,
  mutatedResult,
  calls,
  getterCalls,
  inheritedResult,
  forwardingGetterCalls,
  forwardingResult,
}));
