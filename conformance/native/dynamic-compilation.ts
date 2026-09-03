// What this realm answers when a program asks it to compile source at run time.
//
// It cannot: the native backend emits C ahead of time and there is no compiler in the running
// program (ADR 0030). What it answers is the decision in ADR 0045 -- an `EvalError` the program
// can catch, at the call, rather than a refusal that ends the process.
//
// The kind and the wording are Node's under `--disallow-code-generation-from-strings`, which a
// browser under a Content-Security-Policy also gives. That is the whole point of choosing them: a
// library that already copes with a no-eval environment reaches its own fallback without knowing
// anything about this engine. A bundled `zod` carries a `ZodCompileUnsupportedError` for exactly
// this case and runs natively because of it.
//
// The other half is `EvalError` being a real error at all. It was the one member of the language's
// seven that the backend's error table omitted, so it could not be constructed, had no prototype
// owner, and -- quietly -- could not be thrown: a throw that cannot be built returns a plain error
// status, and the caller reports whatever it was doing instead.

// The throw itself is deliberately NOT asserted here. This corpus compares the two engines, and
// they disagree about `new Function` on purpose: the interpreter is handed a compiler (ADR 0030)
// and compiles it, while the native realm has none and answers the `EvalError`. Asserting either
// answer would make one engine wrong. What the two must agree on is everything below -- that
// `EvalError` is a real error, and that a path not taken costs nothing -- and the throw is covered
// where it matters, by `zod` running under scripts/thirdparty.sh.

// A path that is never taken costs nothing: the answer is at the call, not at emission.
const never = false;
const chosen = never ? new Function("return 1") : () => "fallback";
console.log("unused-path", chosen());

// Every error the language defines, constructed and identified.
for (const name of ["Error", "TypeError", "RangeError", "ReferenceError", "SyntaxError",
                    "URIError", "EvalError"]) {
  const Kind = (globalThis as never as Record<string, new (m: string) => Error>)[name];
  const made = new Kind("m");
  console.log(name, made.constructor.name, made.message, made instanceof Error);
}
