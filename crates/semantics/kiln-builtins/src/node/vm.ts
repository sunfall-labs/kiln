// kiln-builtins: node:vm implementation (Phase 5.2, ADR 0005)

const kilnPrivate: any = (globalThis as any).__kilnPrivate ?? {};

export class Script {
  private _code: string;
  private _options: any;

  constructor(code: string, options?: any) {
    this._code = String(code);
    this._options = options;
  }

  runInThisContext(options?: any): any {
    return runInThisContext(this._code, options);
  }

  runInContext(contextifiedObject: any, options?: any): any {
    return runInContext(this._code, contextifiedObject, options);
  }

  runInNewContext(contextObject?: any, options?: any): any {
    return runInNewContext(this._code, contextObject, options);
  }

  createCachedData(): Uint8Array {
    return new Uint8Array(0);
  }
}

export function createContext(contextObject: any = {}, _options?: any): any {
  return contextObject;
}

export function isContext(object: any): boolean {
  return typeof object === "object" && object !== null;
}

export function runInThisContext(code: string, _options?: any): any {
  return (0, eval)(code);
}

export function runInContext(code: string, contextifiedObject: any, _options?: any): any {
  return runInNewContext(code, contextifiedObject, _options);
}

export function runInNewContext(code: string, contextObject?: any, _options?: any): any {
  const scope = contextObject && typeof contextObject === "object" ? contextObject : {};
  const keys = Object.keys(scope);
  const values = keys.map((k) => scope[k]);
  if (kilnPrivate.gcExposed === true && typeof kilnPrivate.gcRequest === "function" &&
      !keys.includes("gc")) {
    keys.push("gc");
    values.push(kilnPrivate.gcRequest);
  }
  if (keys.length === 0) return (0, eval)(code);
  // Compile expressions as the body of a function whose parameters are the context bindings.
  // The previous indirect eval escaped this scope, making those names invisible to the script.
  // A statement body remains executable through the ordinary Function path.
  let evaluate: any;
  try {
    evaluate = new Function(...keys, `return (${code})`);
  } catch (_error) {
    evaluate = new Function(...keys, code);
  }
  return evaluate(...values);
}

export function createScript(code: string, options?: any): Script {
  return new Script(code, options);
}

export function compileFunction(code: string, params: string[] = [], _options?: any): any {
  return new Function(...params, code);
}

export function measureMemory(_options?: any): Promise<any> {
  return Promise.resolve({
    total: { jsMemoryEstimate: 1024 * 1024 * 16, jsMemoryRange: [0, 1024 * 1024 * 32] },
  });
}

export const constants = {
  DONT_CONTEXTIFY: 1,
  USE_MAIN_CONTEXT_DEFAULT_LOADER: 2,
};

const vmModule = {
  Script,
  createContext,
  createScript,
  runInContext,
  runInNewContext,
  runInThisContext,
  isContext,
  compileFunction,
  measureMemory,
  constants,
  default: undefined as any,
};
vmModule.default = vmModule;

export default vmModule;
