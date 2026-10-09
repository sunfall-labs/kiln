// A statically known arguments[index] read stays observable even when its value is unused.
const token = {};
let inheritedCaught = false;
let inheritedReads = 0;
Object.defineProperty(Object.prototype, "1", {
  configurable: true,
  get() {
    inheritedReads += 1;
    throw token;
  },
});
function readMissingAndDiscard() {
  arguments[1];
}
try {
  readMissingAndDiscard();
} catch (error) {
  inheritedCaught = error === token;
}
delete Object.prototype["1"];

let ownAccessorReads = 0;
function materializeMutateAndDiscard() {
  const mutate = () => {
    Object.defineProperty(arguments, "0", {
      configurable: true,
      get() {
        ownAccessorReads += 1;
        return 41;
      },
    });
  };
  mutate();
  arguments[0];
}
materializeMutateAndDiscard();

console.log(JSON.stringify({ inheritedCaught, inheritedReads, ownAccessorReads }));
