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

// The native host currently exposes directory enumeration as a synchronous filesystem
// capability. Keep the public Promise contract here: invoking the host operation from an async
// function turns both its result and any filesystem error into the corresponding Promise
// fulfillment or rejection. The interpreter/Node adapter's own promise implementation remains
// authoritative when it provides `readdir` directly.
export const readdir: (...args: any[]) => Promise<any[]> =
  nativeModule && typeof nativeModule.readdir === "function"
    ? nativeModule.readdir
    : async function readdir(path: any, options?: any): Promise<any[]> {
        const fs = (process as any).__kilnNativeModules?.fs;
        if (fs && typeof fs.readdirSync === "function") {
          return fs.readdirSync(path, options);
        }
        return unmaterialized("readdir")(path, options);
      };

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
    const BufferClass = typeof Buffer !== "undefined" ? (Buffer as any) : null;
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
  async read<T extends Uint8Array>(
    bufferOrOptions?: T | { buffer?: T; offset?: number; length?: number; position?: number | null },
    offset?: number | null,
    length?: number | null,
    position?: number | null
  ): Promise<{ bytesRead: number; buffer: T }> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (!fs || typeof fs.readSync !== "function") {
      throw new Error("node:fs/promises FileHandle.prototype.read requires an FS host capability that this realm does not provide");
    }
    const BufferClass = typeof Buffer !== "undefined" ? (Buffer as any) : null;
    let targetBuffer: any = bufferOrOptions;
    let targetOffset = offset ?? 0;
    let targetLength = length;
    let targetPosition = position ?? null;

    if (!targetBuffer || (typeof targetBuffer === "object" && !("byteLength" in targetBuffer))) {
      const opts = targetBuffer;
      targetBuffer = opts?.buffer || (BufferClass && typeof BufferClass.alloc === "function" ? BufferClass.alloc(16384) : new Uint8Array(16384));
      targetOffset = opts?.offset ?? 0;
      targetLength = opts?.length ?? (targetBuffer.byteLength - targetOffset);
      targetPosition = opts?.position ?? null;
    } else {
      if (targetLength === undefined || targetLength === null) {
        targetLength = targetBuffer.byteLength - targetOffset;
      }
    }

    const bytesRead = fs.readSync(this.fd, targetBuffer, targetOffset, targetLength, targetPosition);
    return { bytesRead, buffer: targetBuffer };
  }
  async write(
    buffer: any,
    offsetOrPosition?: number | null,
    lengthOrEncoding?: number | string | null,
    position?: number | null
  ): Promise<{ bytesWritten: number; buffer: any }> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (!fs || typeof fs.writeSync !== "function") {
      throw new Error("node:fs/promises FileHandle.prototype.write requires an FS host capability that this realm does not provide");
    }
    if (typeof buffer === "string") {
      const pos = typeof offsetOrPosition === "number" ? offsetOrPosition : null;
      const enc = typeof lengthOrEncoding === "string" ? lengthOrEncoding : "utf8";
      const bytesWritten = fs.writeSync(this.fd, buffer, pos, enc);
      return { bytesWritten, buffer };
    }
    const offset = typeof offsetOrPosition === "number" ? offsetOrPosition : 0;
    const length = typeof lengthOrEncoding === "number" ? lengthOrEncoding : (buffer.byteLength - offset);
    const pos = typeof position === "number" ? position : null;
    const bytesWritten = fs.writeSync(this.fd, buffer, offset, length, pos);
    return { bytesWritten, buffer };
  }
  async chmod(mode: number): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.fchmodSync === "function") {
      fs.fchmodSync(this.fd, mode);
      return;
    }
    throw new Error("node:fs/promises FileHandle.prototype.chmod requires an FS host capability that this realm does not provide");
  }
  async utimes(atime: any, mtime: any): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.futimesSync === "function") {
      fs.futimesSync(this.fd, atime, mtime);
      return;
    }
    throw new Error("node:fs/promises FileHandle.prototype.utimes requires an FS host capability that this realm does not provide");
  }
  async chown(uid: number, gid: number): Promise<void> {
    const fs = (process as any).__kilnNativeModules?.fs;
    if (fs && typeof fs.fchownSync === "function") {
      fs.fchownSync(this.fd, uid, gid);
      return;
    }
  }
}

if (typeof Symbol !== "undefined" && (Symbol as any).asyncDispose) {
  (FileHandle.prototype as any)[(Symbol as any).asyncDispose] = function (this: FileHandle) {
    return this.close();
  };
}

export async function open(path: string, flags: string = "r", mode?: number): Promise<FileHandle> {
  const fs = (process as any).__kilnNativeModules?.fs;
  if (fs && typeof fs.openSync === "function") {
    const fd = fs.openSync(path, flags, mode);
    return new FileHandle(fd);
  }
  throw new Error("node:fs/promises open requires an FS host capability that this realm does not provide");
}

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
