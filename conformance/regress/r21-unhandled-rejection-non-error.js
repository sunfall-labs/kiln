// A reason that is not error-like is reported as node's `UnhandledPromiseRejection`, naming the
// reason the way V8's side-effect-free conversion does.
async function fails() {
  await null;
  throw 42;
}
fails();
console.log("after the call");
