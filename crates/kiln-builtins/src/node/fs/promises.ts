// kiln-builtins: node:fs/promises implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs_promises ||
      (process as any).__kilnNativeModules?.fs?.promises ||
      (typeof (process as any).getBuiltinModule === "function"
        ? (process as any).getBuiltinModule("node:fs/promises") || (process as any).getBuiltinModule("fs/promises")
        : null)
    : null;

export function appendFile(...args: any[]): any {
  if (nativeModule && typeof nativeModule.appendFile === "function") {
    return nativeModule.appendFile(...args);
  }
  throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
}

export function mkdir(...args: any[]): any {
  if (nativeModule && typeof nativeModule.mkdir === "function") {
    return nativeModule.mkdir(...args);
  }
  throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
}

export function readFile(...args: any[]): any {
  if (nativeModule && typeof nativeModule.readFile === "function") {
    return nativeModule.readFile(...args);
  }
  throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
}

export function stat(...args: any[]): any {
  if (nativeModule && typeof nativeModule.stat === "function") {
    return nativeModule.stat(...args);
  }
  throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
}

export function writeFile(...args: any[]): any {
  if (nativeModule && typeof nativeModule.writeFile === "function") {
    return nativeModule.writeFile(...args);
  }
  throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
}

const fsPromisesNamespace = {
  appendFile,
  default: undefined as any,
  mkdir,
  readFile,
  stat,
  writeFile,
};
fsPromisesNamespace.default = fsPromisesNamespace;

export default fsPromisesNamespace;
