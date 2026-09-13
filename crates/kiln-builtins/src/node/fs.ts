// kiln-builtins: node:fs implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs || null
    : null;

const nativePromises =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs_promises ||
      (process as any).__kilnNativeModules?.fs?.promises || null
    : null;

function unmaterialized(name: string): any {
  if (nativeModule && typeof nativeModule[name] === "function") {
    return nativeModule[name];
  }
  return (..._args: any[]) => {
    throw new Error(`node:fs ${name} requires an FS host capability that this realm does not provide`);
  };
}

export function existsSync(...args: any[]): any {
  if (nativeModule && typeof nativeModule.existsSync === "function") {
    return nativeModule.existsSync(...args);
  }
  throw new Error("node:fs requires an FS host capability that this realm does not provide");
}

export const accessSync = unmaterialized("accessSync");
export const appendFileSync = unmaterialized("appendFileSync");
export const closeSync = unmaterialized("closeSync");
export const copyFileSync = unmaterialized("copyFileSync");
export const lstatSync = unmaterialized("lstatSync");
export const mkdirSync = unmaterialized("mkdirSync");
export const openSync = unmaterialized("openSync");
export const readFileSync = unmaterialized("readFileSync");
export const readSync = unmaterialized("readSync");
export const readdirSync = unmaterialized("readdirSync");
export const readlinkSync = unmaterialized("readlinkSync");
export const realpathSync = unmaterialized("realpathSync");
export const renameSync = unmaterialized("renameSync");
export const rmSync = unmaterialized("rmSync");
export const statSync = unmaterialized("statSync");
export const unlinkSync = unmaterialized("unlinkSync");
export const writeFileSync = unmaterialized("writeFileSync");

export const promises: any = nativePromises || {
  appendFile(...args: any[]) {
    if (nativePromises && typeof nativePromises.appendFile === "function") {
      return nativePromises.appendFile(...args);
    }
    throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
  },
  mkdir(...args: any[]) {
    if (nativePromises && typeof nativePromises.mkdir === "function") {
      return nativePromises.mkdir(...args);
    }
    throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
  },
  readFile(...args: any[]) {
    if (nativePromises && typeof nativePromises.readFile === "function") {
      return nativePromises.readFile(...args);
    }
    throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
  },
  stat(...args: any[]) {
    if (nativePromises && typeof nativePromises.stat === "function") {
      return nativePromises.stat(...args);
    }
    throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
  },
  writeFile(...args: any[]) {
    if (nativePromises && typeof nativePromises.writeFile === "function") {
      return nativePromises.writeFile(...args);
    }
    throw new Error("node:fs/promises requires an FS host capability that this realm does not provide");
  },
};

const fsNamespace = {
  accessSync,
  appendFileSync,
  closeSync,
  copyFileSync,
  default: undefined as any,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  promises,
  readFileSync,
  readSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
};
fsNamespace.default = fsNamespace;

export default fsNamespace;
