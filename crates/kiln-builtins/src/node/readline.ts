import { EventEmitter } from "node:events";

export interface Key {
  sequence?: string;
  name?: string;
  ctrl?: boolean;
  meta?: boolean;
  shift?: boolean;
}

export interface ReadLineOptions {
  input: any;
  output?: any;
  completer?: any;
  terminal?: boolean;
  history?: string[];
  historySize?: number;
  prompt?: string;
  crlfDelay?: number;
  removeHistoryDuplicates?: boolean;
  escapeCodeTimeout?: number;
  tabSize?: number;
  signal?: AbortSignal;
}

export class Interface extends EventEmitter {
  public terminal: boolean = false;
  public line: string = "";
  public cursor: number = 0;
  private _prompt: string = "> ";
  private _input: any;
  private _output: any;
  private _closed: boolean = false;
  private _paused: boolean = false;

  constructor(
    options: ReadLineOptions | any,
    output?: any,
    completer?: any,
    terminal?: boolean
  ) {
    super();
    if (options && typeof options.input === "object") {
      this._input = options.input;
      this._output = options.output;
      this.terminal = Boolean(options.terminal);
      if (options.prompt !== undefined) this._prompt = String(options.prompt);
    } else {
      this._input = options;
      this._output = output;
      this.terminal = Boolean(terminal);
    }

    if (this._input) {
      if (typeof this._input.on === "function") {
        this._input.on("data", (chunk: any) => {
          // Data listener
        });
        this._input.on("end", () => {
          this.close();
        });
      }
    }
  }

  setPrompt(prompt: string): void {
    this._prompt = prompt;
  }

  getPrompt(): string {
    return this._prompt;
  }

  prompt(preserveCursor?: boolean): void {
    if (this._output && typeof this._output.write === "function") {
      this._output.write(this._prompt);
    }
  }

  question(query: string, callback?: (answer: string) => void): void {
    if (typeof callback === "function") {
      callback("");
    }
  }

  pause(): this {
    this._paused = true;
    if (this._input && typeof this._input.pause === "function") {
      this._input.pause();
    }
    this.emit("pause");
    return this;
  }

  resume(): this {
    this._paused = false;
    if (this._input && typeof this._input.resume === "function") {
      this._input.resume();
    }
    this.emit("resume");
    return this;
  }

  close(): void {
    if (this._closed) return;
    this._closed = true;
    this.emit("close");
  }

  write(data: any, key?: Key): void {
    // write placeholder
  }

  getCursorPos(): { rows: number; cols: number } {
    return { rows: 0, cols: 0 };
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<string, void, unknown> {
    if (this._input && typeof this._input[Symbol.asyncIterator] === "function") {
      let buffer = "";
      for await (const chunk of this._input) {
        buffer += typeof chunk === "string" ? chunk : chunk.toString("utf8");
        const lines = buffer.split(/\r?\n/);
        buffer = lines.pop() ?? "";
        for (const line of lines) {
          yield line;
        }
      }
      if (buffer.length > 0) {
        yield buffer;
      }
    }
  }
}

export function createInterface(
  options: ReadLineOptions | any,
  output?: any,
  completer?: any,
  terminal?: boolean
): Interface {
  return new Interface(options, output, completer, terminal);
}

export function clearLine(stream: any, dir: number, callback?: () => void): boolean {
  if (typeof callback === "function") callback();
  return true;
}

export function clearScreenDown(stream: any, callback?: () => void): boolean {
  if (typeof callback === "function") callback();
  return true;
}

export function cursorTo(
  stream: any,
  x: number,
  y?: number | (() => void),
  callback?: () => void
): boolean {
  const cb = typeof y === "function" ? y : callback;
  if (typeof cb === "function") cb();
  return true;
}

export function moveCursor(
  stream: any,
  dx: number,
  dy: number,
  callback?: () => void
): boolean {
  if (typeof callback === "function") callback();
  return true;
}

export function emitKeypressEvents(stream: any, readlineInterface?: any): void {
  // hook up keypress events placeholder
}

export const promises = {
  Interface,
  createInterface,
};

const readlineModule = {
  Interface,
  createInterface,
  clearLine,
  clearScreenDown,
  cursorTo,
  moveCursor,
  emitKeypressEvents,
  promises,
  default: undefined as any,
};
readlineModule.default = readlineModule;

export default readlineModule;
