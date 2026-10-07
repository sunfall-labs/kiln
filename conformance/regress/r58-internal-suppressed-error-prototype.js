if (Error.prototype.constructor !== Error ||
    new Error("plain").constructor !== Error ||
    Error.prototype.name !== "Error" ||
    SuppressedError.prototype === Error.prototype) {
  throw new Error("SuppressedError installation changed Error prototype identity");
}

function checkSuppressed(error, first, second, where) {
  if (!(error instanceof SuppressedError) ||
      Object.getPrototypeOf(error) !== SuppressedError.prototype ||
      error.constructor !== SuppressedError ||
      error.error !== first || error.suppressed !== second) {
    throw new Error(`${where} used the wrong SuppressedError prototype or payload`);
  }
}

const syncFirst = { id: "sync first" };
const syncSecond = { id: "sync second" };
const sync = new DisposableStack();
sync.defer(() => { throw syncFirst; });
sync.defer(() => { throw syncSecond; });
try {
  sync.dispose();
  throw new Error("sync disposal did not throw");
} catch (error) {
  checkSuppressed(error, syncFirst, syncSecond, "sync disposal");
}

async function checkAsyncDisposal() {
  const asyncFirst = { id: "async first" };
  const asyncSecond = { id: "async second" };
  const stack = new AsyncDisposableStack();
  stack.defer(() => { throw asyncFirst; });
  stack.defer(() => { throw asyncSecond; });
  try {
    await stack.disposeAsync();
    throw new Error("async disposal did not reject");
  } catch (error) {
    checkSuppressed(error, asyncFirst, asyncSecond, "async disposal");
  }
}

checkAsyncDisposal().then(() => console.log("sync and async SuppressedError prototypes passed"));
