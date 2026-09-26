// kiln-builtins: node:tty implementation (Phase 5.2, ADR 0005)

export function isatty(fd: any): boolean {
  if (typeof fd !== "number" || isNaN(fd) || fd < 0) {
    return false;
  }
  const proc = (globalThis as any).process;
  if (proc) {
    if (fd === 0 && proc.stdin && typeof proc.stdin.isTTY === "boolean") {
      return proc.stdin.isTTY;
    }
    if (fd === 1 && proc.stdout && typeof proc.stdout.isTTY === "boolean") {
      return proc.stdout.isTTY;
    }
    if (fd === 2 && proc.stderr && typeof proc.stderr.isTTY === "boolean") {
      return proc.stderr.isTTY;
    }
  }
  return false;
}

export class ReadStream {
  public isTTY: boolean;
  public isRaw: boolean = false;
  public fd: number;

  constructor(fd: number, options?: any) {
    this.fd = fd;
    this.isTTY = isatty(fd);
  }

  setRawMode(mode: boolean): this {
    this.isRaw = Boolean(mode);
    return this;
  }
}

export class WriteStream {
  public isTTY: boolean;
  public fd: number;
  public columns: number = 80;
  public rows: number = 24;

  constructor(fd: number) {
    this.fd = fd;
    this.isTTY = isatty(fd);
    const proc = (globalThis as any).process;
    if (proc) {
      if (fd === 1 && proc.stdout) {
        if (typeof proc.stdout.columns === "number") this.columns = proc.stdout.columns;
        if (typeof proc.stdout.rows === "number") this.rows = proc.stdout.rows;
      } else if (fd === 2 && proc.stderr) {
        if (typeof proc.stderr.columns === "number") this.columns = proc.stderr.columns;
        if (typeof proc.stderr.rows === "number") this.rows = proc.stderr.rows;
      }
    }
  }

  getColorDepth(env?: any): number {
    return 8;
  }

  hasColors(count?: any, env?: any): boolean {
    return true;
  }

  getWindowSize(): [number, number] {
    return [this.columns, this.rows];
  }

  clearLine(dir: number, callback?: () => void): boolean {
    if (typeof callback === "function") callback();
    return true;
  }

  clearScreenDown(callback?: () => void): boolean {
    if (typeof callback === "function") callback();
    return true;
  }

  cursorTo(x: number, y?: number, callback?: () => void): boolean {
    if (typeof callback === "function") callback();
    return true;
  }

  moveCursor(dx: number, dy: number, callback?: () => void): boolean {
    if (typeof callback === "function") callback();
    return true;
  }
}

const ttyModule = {
  ReadStream,
  WriteStream,
  default: undefined as any,
  isatty,
};
ttyModule.default = ttyModule;

export default ttyModule;
