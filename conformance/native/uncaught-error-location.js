// kiln-native: expect-exit=1
// An error nobody catches ends the run with its stack: function, file, line and column of every
// frame, in the same bytes from both engines. The stdout line proves the program ran first.
console.log("before");
function inner() {
  throw new RangeError("out of range");
}
const holder = {
  method() {
    inner();
  },
};
function outer() {
  holder.method();
}
outer();
