const previousPrepare = Error.prepareStackTrace;
let sites;
Error.prepareStackTrace = (_error, captured) => {
  sites = captured;
  return "captured";
};

const marker = "~effect/Effect/stackProbe";
const probe = {
  [marker]: function stackProbe() {
    return new Error("probe").stack;
  }
};
probe[marker]();
const site = sites?.[0];
if (site?.getFunctionName() !== "stackProbe" ||
    site?.getMethodName() !== marker ||
    !site?.toString().includes(`stackProbe [as ${marker}]`)) {
  throw new Error("Error stack lost the invocation property alias");
}

const sameName = { stackProbe() { return new Error("same").stack; } };
sameName.stackProbe();
const sameSite = sites?.[0];
if (sameSite?.getFunctionName() !== "stackProbe" ||
    sameSite?.getMethodName() !== "stackProbe" ||
    sameSite?.toString().includes("[as stackProbe]")) {
  throw new Error("matching function and method names should not get an alias suffix");
}

const userCode = function userCode() { return new Error("identifier").stack; };
const identifierMethod = { onSuccess: userCode };
identifierMethod.onSuccess();
const identifierSite = sites?.[0];
if (identifierSite?.getFunctionName() !== "userCode" ||
    identifierSite?.getMethodName() !== "onSuccess" ||
    identifierSite?.toString().includes("[as onSuccess]")) {
  throw new Error("identifier properties should retain method metadata without a display alias");
}

Error.prepareStackTrace = previousPrepare;
console.log("error stack method aliases passed");
