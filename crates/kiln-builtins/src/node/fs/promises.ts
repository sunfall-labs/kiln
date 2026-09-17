// kiln-builtins: node:fs/promises implementation (Phase 5.2, ADR 0005)

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs_promises ||
      (process as any).__kilnNativeModules?.fs?.promises || null
    : null;

function unmaterialized(name: string): any {
  if (nativeModule && typeof nativeModule[name] === "function") {
    return nativeModule[name];
  }
  return (..._args: any[]) => {
    return Promise.reject(
      new Error(`node:fs/promises ${name} requires an FS host capability that this realm does not provide`)
    );
  };
}

export const access = unmaterialized("access");
export const appendFile = unmaterialized("appendFile");
export const chmod = unmaterialized("chmod");
export const chown = unmaterialized("chown");
export const copyFile = unmaterialized("copyFile");
export const link = unmaterialized("link");
export const lstat = unmaterialized("lstat");
export const mkdir = unmaterialized("mkdir");
export const mkdtemp = unmaterialized("mkdtemp");
export const open = unmaterialized("open");
export const readdir = unmaterialized("readdir");
export const readFile = unmaterialized("readFile");
export const readlink = unmaterialized("readlink");
export const realpath = unmaterialized("realpath");
export const rename = unmaterialized("rename");
export const rm = unmaterialized("rm");
export const rmdir = unmaterialized("rmdir");
export const stat = unmaterialized("stat");
export const symlink = unmaterialized("symlink");
export const truncate = unmaterialized("truncate");
export const unlink = unmaterialized("unlink");
export const utimes = unmaterialized("utimes");
export const writeFile = unmaterialized("writeFile");

const fsPromisesNamespace = {
  access,
  appendFile,
  chmod,
  chown,
  copyFile,
  default: undefined as any,
  link,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readdir,
  readFile,
  readlink,
  realpath,
  rename,
  rm,
  rmdir,
  stat,
  symlink,
  truncate,
  unlink,
  utimes,
  writeFile,
};
fsPromisesNamespace.default = fsPromisesNamespace;

export default fsPromisesNamespace;
