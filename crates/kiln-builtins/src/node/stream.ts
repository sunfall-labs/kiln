// kiln-builtins: node:stream implementation (Phase 5.2, ADR 0005)

class EventEmitter {
  _events: Record<string | symbol, Function[]>;
  _eventsCount: number;
  _maxListeners: number | undefined;

  constructor() {
    this._events = Object.create(null);
    this._eventsCount = 0;
  }

  on(event: string | symbol, listener: Function): this {
    return this.addListener(event, listener);
  }

  addListener(event: string | symbol, listener: Function): this {
    if (typeof listener !== "function") {
      throw new TypeError("The \"listener\" argument must be of type function.");
    }
    if (!this._events) {
      this._events = Object.create(null);
      this._eventsCount = 0;
    }
    const list = this._events[event];
    if (!list) {
      this._events[event] = [listener];
      this._eventsCount++;
    } else {
      list.push(listener);
    }
    return this;
  }

  once(event: string | symbol, listener: Function): this {
    if (typeof listener !== "function") {
      throw new TypeError("The \"listener\" argument must be of type function.");
    }
    const g = (...args: any[]) => {
      this.removeListener(event, g);
      listener.apply(this, args);
    };
    return this.addListener(event, g);
  }

  removeListener(event: string | symbol, listener: Function): this {
    if (typeof listener !== "function") {
      throw new TypeError("The \"listener\" argument must be of type function.");
    }
    if (!this._events) return this;
    const list = this._events[event];
    if (!list) return this;
    const idx = list.indexOf(listener);
    if (idx !== -1) {
      list.splice(idx, 1);
      if (list.length === 0) {
        delete this._events[event];
        this._eventsCount--;
      }
    }
    return this;
  }

  off(event: string | symbol, listener: Function): this {
    return this.removeListener(event, listener);
  }

  emit(event: string | symbol, ...args: any[]): boolean {
    if (!this._events) return false;
    const list = this._events[event];
    if (!list || list.length === 0) {
      if (event === "error") {
        const err = args[0] || new Error("Unhandled error.");
        throw err;
      }
      return false;
    }
    const copy = list.slice();
    for (let i = 0; i < copy.length; i++) {
      copy[i].apply(this, args);
    }
    return true;
  }

  removeAllListeners(event?: string | symbol): this {
    if (!this._events) return this;
    if (event) {
      if (this._events[event]) {
        delete this._events[event];
        this._eventsCount--;
      }
    } else {
      this._events = Object.create(null);
      this._eventsCount = 0;
    }
    return this;
  }

  listeners(event: string | symbol): Function[] {
    if (!this._events || !this._events[event]) return [];
    return this._events[event].slice();
  }
}

export class Stream extends EventEmitter {
  static Readable: typeof Readable;
  static Writable: typeof Writable;
  static Duplex: typeof Duplex;
  static Transform: typeof Transform;
  static PassThrough: typeof PassThrough;
  static pipeline: typeof pipeline;
  static finished: typeof finished;
  static addAbortSignal: typeof addAbortSignal;
  static Stream: typeof Stream;
  static default: typeof Stream;

  pipe(destination: any, options?: any): any {
    const source = this as any;
    function onData(chunk: any) {
      if (destination.writable !== false) {
        if (false === destination.write(chunk) && source.pause) {
          source.pause();
        }
      }
    }
    source.on("data", onData);
    function onDrain() {
      if (source.readable !== false && source.resume) {
        source.resume();
      }
    }
    destination.on("drain", onDrain);
    if (!destination._isStdio && (!options || options.end !== false)) {
      source.on("end", onEnd);
      source.on("close", onClose);
    }
    let didOnEnd = false;
    function onEnd() {
      if (didOnEnd) return;
      didOnEnd = true;
      destination.end();
    }
    function onClose() {
      if (didOnEnd) return;
      didOnEnd = true;
      if (typeof destination.destroy === "function") {
        destination.destroy();
      }
    }
    source.on("error", (err: any) => destination.emit("error", err));
    destination.on("error", (err: any) => source.emit("error", err));
    return destination;
  }
}

export class Readable extends Stream {
  _readableState: {
    buffer: any[];
    length: number;
    pipes: any[];
    flowing: boolean | null;
    ended: boolean;
    endEmitted: boolean;
    reading: boolean;
    destroyed: boolean;
    highWaterMark: number;
    encoding: string | null;
  };
  readable: boolean;
  readableEnded: boolean;
  destroyed: boolean;

  constructor(options?: any) {
    super();
    const hwm = (options && typeof options.highWaterMark === "number") ? options.highWaterMark : 16 * 1024;
    const enc = (options && typeof options.encoding === "string") ? options.encoding : null;
    this._readableState = {
      buffer: [],
      length: 0,
      pipes: [],
      flowing: null,
      ended: false,
      endEmitted: false,
      reading: false,
      destroyed: false,
      highWaterMark: hwm,
      encoding: enc,
    };
    this.readable = true;
    this.readableEnded = false;
    this.destroyed = false;
    if (options && typeof options.read === "function") {
      (this as any)._read = options.read;
    }
  }

  _read(_size?: number): void {}

  read(size?: number): any {
    const state = this._readableState;
    if (state.buffer.length === 0) {
      if (state.ended && !state.endEmitted) {
        state.endEmitted = true;
        this.readableEnded = true;
        this.emit("end");
      }
      return null;
    }
    if (size === undefined || size >= state.length) {
      const chunks = state.buffer;
      state.buffer = [];
      state.length = 0;
      let res: any;
      if (chunks.length === 1) {
        res = chunks[0];
      } else if (typeof (globalThis as any).Buffer !== "undefined" && typeof (globalThis as any).Buffer.concat === "function") {
        res = (globalThis as any).Buffer.concat(chunks);
      } else {
        res = chunks;
      }
      if (state.ended && !state.endEmitted) {
        state.endEmitted = true;
        this.readableEnded = true;
        this.emit("end");
      }
      return res;
    }
    // Sliced read
    const chunk = state.buffer.shift();
    state.length -= (chunk && chunk.length) || 0;
    return chunk;
  }

  push(chunk: any, encoding?: string): boolean {
    const state = this._readableState;
    if (chunk === null) {
      state.ended = true;
      if (state.length === 0 && !state.endEmitted) {
        state.endEmitted = true;
        this.readableEnded = true;
        this.emit("end");
      }
      return false;
    }
    if (typeof chunk === "string" && state.encoding) {
      // Keep string if encoding configured
    } else if (typeof chunk === "string" && typeof (globalThis as any).Buffer !== "undefined") {
      chunk = (globalThis as any).Buffer.from(chunk, encoding);
    }
    state.buffer.push(chunk);
    state.length += (chunk && chunk.length) || 0;
    this.emit("data", chunk);
    return state.length < state.highWaterMark;
  }

  pause(): this {
    this._readableState.flowing = false;
    this.emit("pause");
    return this;
  }

  resume(): this {
    this._readableState.flowing = true;
    this.emit("resume");
    return this;
  }

  setEncoding(enc: string): this {
    this._readableState.encoding = enc;
    return this;
  }

  destroy(error?: any): this {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._readableState.destroyed = true;
    this.readable = false;
    if (error) this.emit("error", error);
    this.emit("close");
    return this;
  }

  unpipe(destination?: any): this {
    if (!destination) {
      this._readableState.pipes = [];
    } else {
      const idx = this._readableState.pipes.indexOf(destination);
      if (idx !== -1) this._readableState.pipes.splice(idx, 1);
    }
    return this;
  }
}

export class Writable extends Stream {
  _writableState: {
    ended: boolean;
    finished: boolean;
    destroyed: boolean;
    highWaterMark: number;
    decodeStrings: boolean;
  };
  writable: boolean;
  writableEnded: boolean;
  writableFinished: boolean;
  destroyed: boolean;

  constructor(options?: any) {
    super();
    const hwm = (options && typeof options.highWaterMark === "number") ? options.highWaterMark : 16 * 1024;
    this._writableState = {
      ended: false,
      finished: false,
      destroyed: false,
      highWaterMark: hwm,
      decodeStrings: (options && options.decodeStrings !== false),
    };
    this.writable = true;
    this.writableEnded = false;
    this.writableFinished = false;
    this.destroyed = false;
    if (options && typeof options.write === "function") {
      (this as any)._write = options.write;
    }
  }

  _write(chunk: any, encoding: string, callback: (error?: any) => void): void {
    callback();
  }

  write(chunk: any, encoding?: any, callback?: any): boolean {
    if (typeof encoding === "function") {
      callback = encoding;
      encoding = undefined;
    }
    if (this._writableState.ended) {
      const err = new Error("write after end");
      this.emit("error", err);
      if (callback) callback(err);
      return false;
    }
    try {
      this._write(chunk, encoding || "utf8", (err?: any) => {
        if (err) this.emit("error", err);
        if (callback) callback(err);
      });
    } catch (err) {
      this.emit("error", err);
      if (callback) callback(err);
    }
    return true;
  }

  end(chunk?: any, encoding?: any, callback?: any): this {
    if (typeof chunk === "function") {
      callback = chunk;
      chunk = undefined;
    } else if (typeof encoding === "function") {
      callback = encoding;
      encoding = undefined;
    }
    if (chunk) {
      this.write(chunk, encoding);
    }
    this._writableState.ended = true;
    this.writableEnded = true;
    this._writableState.finished = true;
    this.writableFinished = true;
    this.emit("finish");
    if (callback) callback();
    return this;
  }

  destroy(error?: any): this {
    if (this.destroyed) return this;
    this.destroyed = true;
    this._writableState.destroyed = true;
    this.writable = false;
    if (error) this.emit("error", error);
    this.emit("close");
    return this;
  }
}

export class Duplex extends Readable {
  _writableState: any;
  writable: boolean;
  writableEnded: boolean;
  writableFinished: boolean;

  constructor(options?: any) {
    super(options);
    this._writableState = {
      ended: false,
      finished: false,
      destroyed: false,
      highWaterMark: (options && options.highWaterMark) || 16 * 1024,
    };
    this.writable = true;
    this.writableEnded = false;
    this.writableFinished = false;
    if (options && typeof options.write === "function") {
      (this as any)._write = options.write;
    }
  }

  _write(chunk: any, encoding: string, callback: (error?: any) => void): void {
    callback();
  }

  write(chunk: any, encoding?: any, callback?: any): boolean {
    return Writable.prototype.write.call(this, chunk, encoding, callback);
  }

  end(chunk?: any, encoding?: any, callback?: any): this {
    Writable.prototype.end.call(this, chunk, encoding, callback);
    return this;
  }
}

export class Transform extends Duplex {
  constructor(options?: any) {
    super(options);
    if (options && typeof options.transform === "function") {
      (this as any)._transform = options.transform;
    }
  }

  _transform(chunk: any, encoding: string, callback: (error?: any, data?: any) => void): void {
    callback(null, chunk);
  }

  _write(chunk: any, encoding: string, callback: (error?: any) => void): void {
    this._transform(chunk, encoding, (err: any, data: any) => {
      if (err) {
        callback(err);
        return;
      }
      if (data !== undefined && data !== null) {
        this.push(data);
      }
      callback();
    });
  }
}

export class PassThrough extends Transform {}

export function addAbortSignal(signal: any, stream: any): any {
  if (!signal) return stream;
  if (signal.aborted) {
    stream.destroy(signal.reason || new Error("AbortError"));
    return stream;
  }
  const onAbort = () => {
    stream.destroy(signal.reason || new Error("AbortError"));
  };
  if (typeof signal.addEventListener === "function") {
    signal.addEventListener("abort", onAbort, { once: true });
  } else if (typeof signal.once === "function") {
    signal.once("abort", onAbort);
  }
  return stream;
}

export function finished(stream: any, optionsOrCb: any, cb?: any): Function {
  const callback = typeof optionsOrCb === "function" ? optionsOrCb : cb;
  if (typeof callback !== "function") {
    throw new TypeError("The \"callback\" argument must be of type function.");
  }
  let closed = false;
  const onFinish = () => {
    if (closed) return;
    closed = true;
    cleanup();
    callback();
  };
  const onError = (err: any) => {
    if (closed) return;
    closed = true;
    cleanup();
    callback(err);
  };
  function cleanup() {
    stream.removeListener("finish", onFinish);
    stream.removeListener("end", onFinish);
    stream.removeListener("close", onFinish);
    stream.removeListener("error", onError);
  }
  stream.on("finish", onFinish);
  stream.on("end", onFinish);
  stream.on("close", onFinish);
  stream.on("error", onError);
  return cleanup;
}

export function pipeline(...args: any[]): any {
  const callback = typeof args[args.length - 1] === "function" ? args.pop() : undefined;
  const streams = args;
  if (streams.length < 2) {
    throw new Error("pipeline requires at least two streams");
  }
  for (let i = 0; i < streams.length - 1; i++) {
    streams[i].pipe(streams[i + 1]);
  }
  if (callback) {
    finished(streams[streams.length - 1], callback);
  }
  return streams[streams.length - 1];
}

Stream.Readable = Readable;
Stream.Writable = Writable;
Stream.Duplex = Duplex;
Stream.Transform = Transform;
Stream.PassThrough = PassThrough;
Stream.pipeline = pipeline;
Stream.finished = finished;
Stream.addAbortSignal = addAbortSignal;
Stream.Stream = Stream;
Stream.default = Stream;

export default Stream;
