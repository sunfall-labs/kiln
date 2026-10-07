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
  const hasSyntheticGc = kilnPrivate.gcExposed === true &&
    typeof kilnPrivate.gcRequest === "function" && !("gc" in scope);
  // A with-environment gives direct eval an object environment: reads fall through to the
  // realm, while writes to existing context properties update the caller's context object.
  // Keep the optional gc binding in the outer function environment so the context object stays
  // untouched and an own context property still takes precedence.
  const body = `with (environment) { return eval(${JSON.stringify(code)}); }`;
  if (hasSyntheticGc) {
    const evaluate = new Function("environment", "gc", body);
    return evaluate(scope, kilnPrivate.gcRequest);
  }
  const evaluate = new Function("environment", body);
  return evaluate(scope);
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
