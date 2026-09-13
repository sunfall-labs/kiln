// kiln-builtins: node:ffi implementation (Phase 5.2, ADR 0005)

const nativeModules =
  typeof process !== "undefined" ? (process as any).__kilnNativeModules : undefined;
const nativeModule = nativeModules
  ? nativeModules.ffi || null
  : null;

export const suffix: string =
  typeof nativeModule?.suffix === "string"
    ? nativeModule.suffix
    : typeof process !== "undefined" && process.platform === "win32"
      ? ".dll"
      : typeof process !== "undefined" && process.platform === "darwin"
        ? ".dylib"
        : ".so";

export function dlopen(...args: any[]): any {
  if (nativeModule && typeof nativeModule.dlopen === "function") {
    return nativeModule.dlopen(...args);
  }
  throw new Error("node:ffi requires an FFI host capability that this realm does not provide");
}

export function getRawPointer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.getRawPointer === "function") {
    return nativeModule.getRawPointer(...args);
  }
  throw new Error("node:ffi requires an FFI host capability that this realm does not provide");
}

export function toArrayBuffer(...args: any[]): any {
  if (nativeModule && typeof nativeModule.toArrayBuffer === "function") {
    return nativeModule.toArrayBuffer(...args);
  }
  throw new Error("node:ffi requires an FFI host capability that this realm does not provide");
}

const ffiNamespace = {
  default: undefined as any,
  dlopen,
  getRawPointer,
  suffix,
  toArrayBuffer,
};
ffiNamespace.default = ffiNamespace;

export default ffiNamespace;
