// Errors the host raises are DOMExceptions even when the program never names one: an abort's
// default reason, AbortSignal.timeout's, and structuredClone's refusal. The native executable
// keeps the WHATWG platform for these programs.
const controller = new AbortController();
controller.abort();
const reason = controller.signal.reason;
console.log(reason.name, Object.getPrototypeOf(reason).constructor.name, String(reason));
try {
  structuredClone(() => 1);
} catch (error) {
  console.log(error.name, Object.getPrototypeOf(error).constructor.name);
}
const timeout = AbortSignal.timeout(1);
timeout.addEventListener("abort", () => {
  console.log(timeout.reason.name, Object.getPrototypeOf(timeout.reason).constructor.name);
});
