// kiln-builtins: node:fs implementation (Phase 5.2, ADR 0005)

import { FileHandle, open as openPromise } from "node:fs/promises";

const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs || null
    : null;

const nativePromises =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.fs_promises ||
      (process as any).__kilnNativeModules?.fs?.promises || null
    : null;

// The native backend's private OS descriptor operations. This is a byte/handle
// transport; public overloads, conversion and callback timing stay in TypeScript.
const descriptorOp: ((op: number, a: any, b?: any, c?: any) => any) | undefined =
  (process as any).__kilnFsDescriptor;
delete (process as any).__kilnFsDescriptor;

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
export function closeSync(fd: any): void {
  if (nativeModule && typeof nativeModule.closeSync === "function") return nativeModule.closeSync(fd);
  if (typeof descriptorOp !== "function") return unmaterialized("closeSync")(fd);
  descriptorOp(3, Number(fd));
}
export const copyFileSync = unmaterialized("copyFileSync");
export const lstatSync = unmaterialized("lstatSync");
export const mkdirSync = unmaterialized("mkdirSync");
export function openSync(path: any, flags: any = "r", mode: any = 0o666): number {
  if (nativeModule && typeof nativeModule.openSync === "function") return nativeModule.openSync(path, flags, mode);
  if (typeof descriptorOp !== "function") return unmaterialized("openSync")(path, flags, mode);
  const names = ["r", "r+", "w", "wx", "w+", "wx+", "a", "ax", "a+", "ax+"];
  const alias: Record<string, string> = { rs: "r", "rs+": "r+", xw: "wx", "xw+": "wx+", xa: "ax", "xa+": "ax+" };
  const spelling = String(flags);
  const which = names.indexOf(alias[spelling] || spelling);
  if (which < 0) throw new TypeError(`Unknown file open flag: ${spelling}`);
  return descriptorOp(1, String(path), which, Number(mode));
}
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
// Composed from the host's primitives when the host has no `rmSync` of its own: a directory is
// emptied through `readdirSync` and dropped with `rmdirSync`, anything else through
// `unlinkSync`; `force` swallows a missing path, `recursive` is what makes a directory legal.
export const rmSync =
  nativeModule && typeof nativeModule.rmSync === "function"
    ? nativeModule.rmSync
    : function rmSync(target: any, options?: any): void {
        const path = String(target);
        const recursive = !!(options && options.recursive);
        const force = !!(options && options.force);
        let stat: any;
        try {
          stat = lstatSync(path);
        } catch (e: any) {
          if (force) return;
          throw e;
        }
        if (stat.isDirectory()) {
          if (!recursive) {
            const err: any = new Error(`Path is a directory: rm returned EISDIR (is a directory) ${path}`);
            err.code = "ERR_FS_EISDIR";
            err.path = path;
            throw err;
          }
          for (const entry of readdirSync(path)) rmSync(path + "/" + String(entry), { recursive: true, force });
          rmdirSync(path);
        } else {
          unlinkSync(path);
        }
      };
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
// Descriptor callbacks run in a later turn. The host keeps the descriptor table; these adapters
// preserve Node's `(error, fd)` and `(error, bytesWritten, data)` callback shapes without giving
// user code a raw OS descriptor or treating the synchronous result as the API's return value.
function descriptorCallback(callback: any, operation: () => any, done: (value: any) => void): void {
  if (typeof callback !== "function") {
    throw new TypeError('The "callback" argument must be of type function');
  }
  setImmediate(() => {
    let value: any;
    try {
      value = operation();
    } catch (error) {
      callback(error);
      return;
    }
    done(value);
  });
}

export function open(path: any, flags: any, modeOrCallback: any, callback?: any): void {
  const mode = typeof modeOrCallback === "function" ? undefined : modeOrCallback;
  const done = typeof modeOrCallback === "function" ? modeOrCallback : callback;
  descriptorCallback(done, () => openSync(path, flags, mode), (fd) => done(null, fd));
}

export function write(fd: number, data: any, ...rest: any[]): void {
  const callback = rest.pop();
  const isText = typeof data === "string";
  const position = isText && typeof rest[0] === "number" ? rest[0] : undefined;
  const offset = typeof rest[0] === "number" ? rest[0] : 0;
  const length = typeof rest[1] === "number" ? rest[1] : (data?.byteLength ?? data?.length);
  const bufferPosition = typeof rest[2] === "number" ? rest[2] : null;
  descriptorCallback(
    callback,
    () => isText
      ? writeSync(fd, data, position)
      : writeSync(fd, data, offset, length, bufferPosition),
    (written) => callback(null, written, data),
  );
}

export function close(fd: number, callback: any): void {
  descriptorCallback(callback, () => closeSync(fd), () => callback(null));
}

export function fsync(fd: number, callback: any): void {
  descriptorCallback(callback, () => fsyncSync(fd), () => callback(null));
}
export const fchmodSync = unmaterialized("fchmodSync");
export const futimesSync = unmaterialized("futimesSync");
export const fchmod = makeCallback(fchmodSync);
export const futimes = makeCallback(futimesSync);

export const promises: any = {
  FileHandle,
  open: openPromise,
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
export function fsyncSync(fd: any): void {
  if (nativeModule && typeof nativeModule.fsyncSync === "function") return nativeModule.fsyncSync(fd);
  if (typeof descriptorOp !== "function") return unmaterialized("fsyncSync")(fd);
  descriptorOp(4, Number(fd));
}
export const ftruncateSync = unmaterialized("ftruncateSync");
export const globSync = unmaterialized("globSync");
// `prefix` plus six characters of `[a-z0-9]`, retried while the directory already exists.
export const mkdtempSync =
  nativeModule && typeof nativeModule.mkdtempSync === "function"
    ? nativeModule.mkdtempSync
    : function mkdtempSync(prefix: any): string {
        const alphabet = "abcdefghijklmnopqrstuvwxyz0123456789";
        for (let attempt = 0; attempt < 100; attempt++) {
          let suffix = "";
          for (let i = 0; i < 6; i++) suffix += alphabet[Math.floor(Math.random() * alphabet.length)];
          const path = String(prefix) + suffix;
          if (existsSync(path)) continue;
          mkdirSync(path);
          return path;
        }
        throw new Error(`mkdtemp: could not find a free name for ${String(prefix)}`);
      };
export const truncateSync = unmaterialized("truncateSync");
export const unwatchFile = unmaterialized("unwatchFile");
export const watch = unmaterialized("watch");
export const watchFile = unmaterialized("watchFile");
export function writeSync(fd: any, data: any, offsetOrPosition?: any,
                          lengthOrEncoding?: any, position?: any): number {
  if (nativeModule && typeof nativeModule.writeSync === "function") {
    return nativeModule.writeSync(fd, data, offsetOrPosition, lengthOrEncoding, position);
  }
  if (typeof descriptorOp !== "function") {
    return unmaterialized("writeSync")(fd, data, offsetOrPosition, lengthOrEncoding, position);
  }
  let bytes: Uint8Array;
  let at = -1;
  if (typeof data === "string") {
    const encoding = typeof offsetOrPosition === "string" ? offsetOrPosition
      : typeof lengthOrEncoding === "string" ? lengthOrEncoding : "utf8";
    bytes = Buffer.from(data, encoding);
    if (typeof offsetOrPosition === "number") at = offsetOrPosition;
  } else if (ArrayBuffer.isView(data) && !(data instanceof DataView)) {
    const offset = offsetOrPosition === undefined ? 0 : Number(offsetOrPosition);
    const length = lengthOrEncoding === undefined ? data.byteLength - offset : Number(lengthOrEncoding);
    if (!Number.isInteger(offset) || !Number.isInteger(length) || offset < 0 || length < 0 || offset + length > data.byteLength) {
      throw new RangeError("The value of offset or length is out of range");
    }
    bytes = new Uint8Array(data.buffer, data.byteOffset + offset, length);
    if (typeof position === "number") at = position;
  } else {
    throw new TypeError("The \"buffer\" argument must be a string or an ArrayBuffer view");
  }
  return descriptorOp(2, Number(fd), bytes, at);
}

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
  close,
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
  fsync,
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
  open,
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
  write,
  writeSync,
};
fsNamespace.default = fsNamespace;

export default fsNamespace;
