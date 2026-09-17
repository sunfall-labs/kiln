// kiln-builtins: node:timers/promises implementation (Phase 5.2, ADR 0005)

export function setTimeout(delay = 0, value?: any, options?: { signal?: AbortSignal }): Promise<any> {
  return new Promise((resolve, reject) => {
    if (options?.signal?.aborted) {
      return reject(options.signal.reason);
    }
    let timer: any;
    let onAbort: (() => void) | undefined;
    if (options?.signal) {
      onAbort = () => {
        globalThis.clearTimeout(timer);
        reject(options.signal!.reason);
      };
      options.signal.addEventListener("abort", onAbort, { once: true });
    }
    timer = globalThis.setTimeout(() => {
      if (options?.signal && onAbort) {
        options.signal.removeEventListener("abort", onAbort);
      }
      resolve(value);
    }, delay);
  });
}

export function setImmediate(value?: any, options?: { signal?: AbortSignal }): Promise<any> {
  return setTimeout(0, value, options);
}

export async function* setInterval(delay = 0, value?: any, options?: { signal?: AbortSignal }): AsyncGenerator<any, void, undefined> {
  while (true) {
    await setTimeout(delay, undefined, options);
    yield value;
  }
}

const timersPromisesNamespace = {
  default: undefined as any,
  setImmediate,
  setInterval,
  setTimeout,
};
timersPromisesNamespace.default = timersPromisesNamespace;

export default timersPromisesNamespace;
