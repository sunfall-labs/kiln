// Kiln's compiler-owned implementation of Oxc's explicit-resource-management helper.
// Its call contract follows Babel's MIT-licensed `usingCtx` helper, which Oxc's runtime package
// copies. Keeping the helper in the typed module graph avoids an undeclared npm dependency.
type Resource = { value: any; dispose: any; async: boolean };

const EMPTY_ERROR = {};

function suppressedError(error: any, suppressed: any): any {
  const Ctor = (globalThis as any).SuppressedError;
  if (typeof Ctor === "function") return new Ctor(error, suppressed);
  const result = new Error("An error was suppressed during disposal") as any;
  result.name = "SuppressedError";
  result.error = error;
  result.suppressed = suppressed;
  return result;
}

function disposalSymbol(name: string, fallback: string): symbol {
  const candidate = (Symbol as any)[name];
  return typeof candidate === "symbol" ? candidate : Symbol.for(fallback);
}

export default function usingCtx(): any {
  const stack: Resource[] = [];

  function register(value: any, isAsync: boolean): any {
    if (value != null) {
      if (Object(value) !== value) {
        throw new TypeError(
          "using declarations can only be used with objects, functions, null, or undefined.",
        );
      }

      let dispose: any;
      if (isAsync) dispose = value[disposalSymbol("asyncDispose", "Symbol.asyncDispose")];
      let asyncFallback: any;
      if (dispose === undefined || dispose === null) {
        dispose = value[disposalSymbol("dispose", "Symbol.dispose")];
        if (isAsync) asyncFallback = dispose;
      }
      if (typeof dispose !== "function") throw new TypeError("Object is not disposable.");
      if (asyncFallback) {
        const syncDispose = asyncFallback;
        dispose = function () {
          try {
            syncDispose.call(value);
          } catch (error) {
            return Promise.reject(error);
          }
        };
      }
      stack.push({ value, dispose, async: isAsync });
    } else if (isAsync) {
      // Nullish `await using` still creates an asynchronous disposal boundary.
      stack.push({ value, dispose: value, async: true });
    }
    return value;
  }

  const context: any = {
    e: EMPTY_ERROR,
    u(value: any) {
      return register(value, false);
    },
    a(value: any) {
      return register(value, true);
    },
    d() {
      let error = this.e;
      let needsAwait = false;
      let hasAwaited = false;

      const recordError = (nextError: any): any => {
        error = error !== EMPTY_ERROR ? suppressedError(nextError, error) : nextError;
        return next();
      };

      const next = (): any => {
        let resource: Resource | undefined;
        while ((resource = stack.pop())) {
          try {
            if (!resource.async && needsAwait && !hasAwaited) {
              needsAwait = false;
              hasAwaited = true;
              stack.push(resource);
              return Promise.resolve().then(next);
            }
            if (resource.dispose) {
              const result = resource.dispose.call(resource.value);
              if (resource.async) {
                return Promise.resolve(result).then(
                  () => {
                    hasAwaited = true;
                    return next();
                  },
                  (nextError) => {
                    hasAwaited = true;
                    return recordError(nextError);
                  },
                );
              }
            } else {
              needsAwait = true;
            }
          } catch (nextError) {
            return recordError(nextError);
          }
        }
        if (needsAwait && !hasAwaited) {
          return error !== EMPTY_ERROR ? Promise.reject(error) : Promise.resolve();
        }
        if (error !== EMPTY_ERROR) throw error;
      };

      return next();
    },
  };
  return context;
}
