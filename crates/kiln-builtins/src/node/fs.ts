// kiln-builtins: node:fs implementation (Phase 5.2, ADR 0005)

import { FileHandle, open } from "node:fs/promises";

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
const _realpathSync = unmaterialized("realpathSync");
export function realpathSync(...args: any[]): any {
  return _realpathSync(...args);
}
(realpathSync as any).native = realpathSync;
export const renameSync = unmaterialized("renameSync");
export const rmSync = unmaterialized("rmSync");
export const rmdirSync = unmaterialized("rmdirSync");
export const statSync = unmaterialized("statSync");
export const unlinkSync = unmaterialized("unlinkSync");
export const utimesSync = unmaterialized("utimesSync");
export const writeFileSync = unmaterialized("writeFileSync");

function makeCallback(syncFn: any) {
  return (...args: any[]) => {
    const cb = typeof args[args.length - 1] === "function" ? args.pop() : null;
    try {
      const res = syncFn(...args);
      if (cb) {
        if (typeof process !== "undefined" && typeof process.nextTick === "function") {
          process.nextTick(() => cb(null, res));
        } else {
          cb(null, res);
        }
      }
      return res;
    } catch (err) {
      if (cb) {
        if (typeof process !== "undefined" && typeof process.nextTick === "function") {
          process.nextTick(() => cb(err));
        } else {
          cb(err);
        }
      } else {
        throw err;
      }
    }
  };
}

export const rmdir = makeCallback(rmdirSync);
export const utimes = makeCallback(utimesSync);
export const mkdir = makeCallback(mkdirSync);
export const stat = makeCallback(statSync);
export const lstat = makeCallback(lstatSync);
export const realpath = makeCallback(realpathSync);
export const readFile = makeCallback(readFileSync);
export const writeFile = makeCallback(writeFileSync);
export const unlink = makeCallback(unlinkSync);
export const fchmodSync = unmaterialized("fchmodSync");
export const futimesSync = unmaterialized("futimesSync");
export const fchmod = makeCallback(fchmodSync);
export const futimes = makeCallback(futimesSync);

export const promises: any = {
  FileHandle,
  open,
  access: nativePromises?.access || unmaterialized("access"),
  appendFile: nativePromises?.appendFile || unmaterialized("appendFile"),
  chmod: nativePromises?.chmod || unmaterialized("chmod"),
  chown: nativePromises?.chown || unmaterialized("chown"),
  copyFile: nativePromises?.copyFile || unmaterialized("copyFile"),
  cp: nativePromises?.cp || unmaterialized("cp"),
  glob: nativePromises?.glob || unmaterialized("glob"),
  link: nativePromises?.link || unmaterialized("link"),
  lstat: nativePromises?.lstat || unmaterialized("lstat"),
  mkdir: nativePromises?.mkdir || unmaterialized("mkdir"),
  mkdtemp: nativePromises?.mkdtemp || unmaterialized("mkdtemp"),
  readdir: nativePromises?.readdir || unmaterialized("readdir"),
  readFile: nativePromises?.readFile || unmaterialized("readFile"),
  readlink: nativePromises?.readlink || unmaterialized("readlink"),
  realpath: nativePromises?.realpath || unmaterialized("realpath"),
  rename: nativePromises?.rename || unmaterialized("rename"),
  rm: nativePromises?.rm || unmaterialized("rm"),
  rmdir: nativePromises?.rmdir || unmaterialized("rmdir"),
  stat: nativePromises?.stat || unmaterialized("stat"),
  symlink: nativePromises?.symlink || unmaterialized("symlink"),
  truncate: nativePromises?.truncate || unmaterialized("truncate"),
  unlink: nativePromises?.unlink || unmaterialized("unlink"),
  utimes: nativePromises?.utimes || unmaterialized("utimes"),
  watch: nativePromises?.watch || unmaterialized("watch"),
  writeFile: nativePromises?.writeFile || unmaterialized("writeFile"),
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
  O_SYMLINK: 2097152,
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
export const fdatasyncSync = unmaterialized("fdatasyncSync");
export const fsyncSync = unmaterialized("fsyncSync");
export const ftruncateSync = unmaterialized("ftruncateSync");
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
  fchmod,
  fchmodSync,
  fdatasyncSync,
  fstatSync,
  fsyncSync,
  ftruncateSync,
  futimes,
  futimesSync,
  glob,
  globSync,
  lstat,
  lstatSync,
  mkdir,
  mkdirSync,
  mkdtempSync,
  openSync,
  promises,
  readFile,
  readFileSync,
  readSync,
  readdirSync,
  readlinkSync,
  realpath,
  realpathSync,
  renameSync,
  rmSync,
  rmdir,
  rmdirSync,
  stat,
  statSync,
  truncateSync,
  unlink,
  unlinkSync,
  unwatchFile,
  utimes,
  utimesSync,
  watch,
  watchFile,
  writeFile,
  writeFileSync,
  writeSync,
};
fsNamespace.default = fsNamespace;

export default fsNamespace;
