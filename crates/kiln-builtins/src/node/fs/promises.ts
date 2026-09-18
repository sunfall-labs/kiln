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
export const cp = unmaterialized("cp");
export const glob = unmaterialized("glob");
export const link = unmaterialized("link");
export const lstat = unmaterialized("lstat");
export const mkdir = unmaterialized("mkdir");
export const mkdtemp = unmaterialized("mkdtemp");

export class FileHandle {
  readonly fd: number;
  constructor(fd: number) {
    this.fd = fd;
  }
  async close(): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.closeSync === "function") {
      fs.closeSync(this.fd);
    }
  }
  async stat(): Promise<any> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.fstatSync === "function") {
      return fs.fstatSync(this.fd);
    }
  }
  async readFile(options?: any): Promise<any> {
    const fs = (process as any).__kilnNativeModules?.fs;
    const BufferClass = (globalThis as any).Buffer;
    if (fs && typeof fs.fstatSync === "function" && typeof fs.readSync === "function") {
      const stats = fs.fstatSync(this.fd);
      const size = (stats && typeof stats.size === "number") ? stats.size : 0;
      const buf = BufferClass && typeof BufferClass.alloc === "function" ? BufferClass.alloc(size) : new Uint8Array(size);
      fs.readSync(this.fd, buf, 0, size, 0);
      const enc = typeof options === "string" ? options : options?.encoding;
      if (enc && typeof (buf as any).toString === "function") {
        return (buf as any).toString(enc);
      }
      return buf;
    }
  }
  async writeFile(data: any, options?: any): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.writeSync === "function") {
      fs.writeSync(this.fd, data);
    }
  }
  async appendFile(data: any, options?: any): Promise<void> {
    return this.writeFile(data, options);
  }
  async truncate(len: number = 0): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.ftruncateSync === "function") {
      fs.ftruncateSync(this.fd, len);
    }
  }
  async sync(): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.fsyncSync === "function") {
      fs.fsyncSync(this.fd);
    }
  }
  async datasync(): Promise<void> {
    return this.sync();
  }
}

export async function open(path: string, flags: string = "r", mode?: number): Promise<FileHandle> {
  const fs = (process as any).__kilnNativeModules?.fs;
  if (fs && typeof fs.openSync === "function") {
    const fd = fs.openSync(path, flags, mode);
    return new FileHandle(fd);
  }
  throw new Error("node:fs/promises open requires an FS host capability that this realm does not provide");
}

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
export const watch = unmaterialized("watch");
export const writeFile = unmaterialized("writeFile");

const fsPromisesNamespace = {
  FileHandle,
  access,
  appendFile,
  chmod,
  chown,
  copyFile,
  cp,
  default: undefined as any,
  glob,
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
  watch,
  writeFile,
};
fsPromisesNamespace.default = fsPromisesNamespace;

export default fsPromisesNamespace;
