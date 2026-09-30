Error.prepareStackTrace = (_error, sites) => sites;

function inner() {
  const error = { name: "ExampleError", message: "captured" };
  Error.captureStackTrace(error);
  return error.stack;
}

function outer() {
  return inner();
}

const full = outer();
console.log(
  full[0].getFunctionName(),
  typeof full[0].getFileName() === "string",
  Object.keys(full[0]).length,
);

function trimHere() {
  const error = {};
  Error.captureStackTrace(error, trimHere);
  return error.stack;
}

function caller() {
  return trimHere();
}

const trimmed = caller();
console.log(
  trimmed[0].getFunctionName(),
  typeof trimmed[0].getFileName() === "string",
);

const nativeCaptureStackTrace = Error.captureStackTrace;
let overrideCalls = 0;
Error.captureStackTrace = (target) => {
  overrideCalls++;
  target.stack = "custom capture";
};
const customTarget = {};
Error.captureStackTrace(customTarget);
Error.captureStackTrace = nativeCaptureStackTrace;
console.log(overrideCalls, customTarget.stack);
