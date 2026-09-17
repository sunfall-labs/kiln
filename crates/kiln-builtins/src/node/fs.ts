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
export const chmodSync = unmaterialized("chmodSync");
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
  access: unmaterialized("access"),
  appendFile: unmaterialized("appendFile"),
  chmod: unmaterialized("chmod"),
  chown: unmaterialized("chown"),
  copyFile: unmaterialized("copyFile"),
  link: unmaterialized("link"),
  lstat: unmaterialized("lstat"),
  mkdir: unmaterialized("mkdir"),
  mkdtemp: unmaterialized("mkdtemp"),
  open: unmaterialized("open"),
  readdir: unmaterialized("readdir"),
  readFile: unmaterialized("readFile"),
  readlink: unmaterialized("readlink"),
  realpath: unmaterialized("realpath"),
  rename: unmaterialized("rename"),
  rm: unmaterialized("rm"),
  rmdir: unmaterialized("rmdir"),
  stat: unmaterialized("stat"),
  symlink: unmaterialized("symlink"),
  truncate: unmaterialized("truncate"),
  unlink: unmaterialized("unlink"),
  utimes: unmaterialized("utimes"),
  writeFile: unmaterialized("writeFile"),
};

export const constants = {
  F_OK: 0,
  R_OK: 4,
  W_OK: 2,
  X_OK: 1,
  COPYFILE_EXCL: 1,
  COPYFILE_FICLONE: 2,
  COPYFILE_FICLONE_FORCE: 4,
  O_RDONLY: 0,
  O_WRONLY: 1,
  O_RDWR: 2,
  O_CREAT: 64,
  O_EXCL: 128,
  O_NOCTTY: 256,
  O_TRUNC: 512,
  O_APPEND: 1024,
  O_DIRECTORY: 65536,
  O_NOFOLLOW: 131072,
  O_SYNC: 1052672,
  O_DSYNC: 4096,
  O_NONBLOCK: 2048,
  S_IFMT: 61440,
  S_IFREG: 32768,
  S_IFDIR: 16384,
  S_IFCHR: 8192,
  S_IFBLK: 24576,
  S_IFIFO: 4096,
  S_IFLNK: 40960,
  S_IFSOCK: 49152,
  S_IRWXU: 448,
  S_IRUSR: 256,
  S_IWUSR: 128,
  S_IXUSR: 64,
  S_IRWXG: 56,
  S_IRGRP: 32,
  S_IWGRP: 16,
  S_IXGRP: 8,
  S_IRWXO: 7,
  S_IROTH: 4,
  S_IWOTH: 2,
  S_IXOTH: 1,
};

export const createReadStream = unmaterialized("createReadStream");
export const createWriteStream = unmaterialized("createWriteStream");
export const fstatSync = unmaterialized("fstatSync");
export const glob = unmaterialized("glob");
export const globSync = unmaterialized("globSync");
export const mkdtempSync = unmaterialized("mkdtempSync");
export const truncateSync = unmaterialized("truncateSync");
export const unwatchFile = unmaterialized("unwatchFile");
export const watch = unmaterialized("watch");
export const watchFile = unmaterialized("watchFile");
export const writeSync = unmaterialized("writeSync");

export class FSWatcher {
  close(): void {}
  ref(): this { return this; }
  unref(): this { return this; }
  on(..._args: any[]): this { return this; }
  once(..._args: any[]): this { return this; }
  emit(..._args: any[]): boolean { return false; }
}

export class Stats {
  isFile(): boolean { return true; }
  isDirectory(): boolean { return false; }
  isSymbolicLink(): boolean { return false; }
  isBlockDevice(): boolean { return false; }
  isCharacterDevice(): boolean { return false; }
  isFIFO(): boolean { return false; }
  isSocket(): boolean { return false; }
  size: number = 0;
  mtimeMs: number = 0;
  ctimeMs: number = 0;
  atimeMs: number = 0;
  birthtimeMs: number = 0;
  mode: number = 0;
}

const fsNamespace = {
  FSWatcher,
  Stats,
  accessSync,
  appendFileSync,
  chmodSync,
  closeSync,
  constants,
  copyFileSync,
  createReadStream,
  createWriteStream,
  default: undefined as any,
  existsSync,
  fstatSync,
  glob,
  globSync,
  lstatSync,
  mkdirSync,
  mkdtempSync,
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
  truncateSync,
  unlinkSync,
  unwatchFile,
  watch,
  watchFile,
  writeFileSync,
  writeSync,
};
fsNamespace.default = fsNamespace;

export default fsNamespace;
