// kiln-native: expect-exit=1
// A rejection nobody handled ends the run like a throw, marked `(in promise)`.
async function fail() {
  await null;
  throw new Error("rejected later");
}
console.log("before");
fail();
