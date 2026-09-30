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

  eventNames(): (string | symbol)[] {
    if (!this._events || this._eventsCount === 0) return [];
    const keys = Object.keys(this._events);
    const symbols = typeof Object.getOwnPropertySymbols === "function"
      ? Object.getOwnPropertySymbols(this._events)
      : [];
    const all: (string | symbol)[] = (keys as (string | symbol)[]).concat(symbols);
    return all.filter((key) => this._events[key] !== undefined);
  }
}

// Node exposes EventEmitter.prototype.on as an alias of addListener. Keep the alias itself
// instead of delegating through `this.addListener`: legacy stream subclasses commonly replace
// `addListener` with their `on` wrapper, which calls Stream.prototype.on.call(this, ...). A
// dynamic delegation here would recurse through that wrapper forever.
EventEmitter.prototype.on = EventEmitter.prototype.addListener;

class StreamImplementation extends EventEmitter {
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

// readable-stream and other legacy subclasses initialize their receiver with Stream.call(this).
export function Stream(this: any, options?: any): void {
  if (this._events === undefined) {
    this._events = Object.create(null);
    this._eventsCount = 0;
  }
  this._maxListeners = this._maxListeners || undefined;
}
export type Stream = StreamImplementation;
Stream.prototype = StreamImplementation.prototype;
Object.defineProperty(Stream.prototype, "constructor", {
  value: Stream, writable: true, configurable: true,
});

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
    objectMode: boolean;
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
      objectMode: !!(options && (options.objectMode || options.readableObjectMode)),
    };
    (this as any).readableObjectMode = this._readableState.objectMode;
    this.readable = true;
    this.readableEnded = false;
    this.destroyed = false;
    if (options && typeof options.read === "function") {
      (this as any)._read = options.read;
    }
  }

  addListener(event: string | symbol, listener: Function): this {
    super.addListener(event, listener);
    if (event === "data") {
      // A `data` listener puts the stream in flowing mode, as in node: `resume()` starts the
      // reads, and each chunk goes to the listeners instead of the buffer.
      if (this._readableState.flowing !== false) this.resume();
    }
    if (event === "readable") {
      const state = this._readableState;
      if (state.length > 0 || state.ended) {
        if (typeof process !== "undefined" && typeof process.nextTick === "function") {
          process.nextTick(() => {
            if (!this.destroyed) this.emit("readable");
          });
        } else {
          setTimeout(() => {
            if (!this.destroyed) this.emit("readable");
          }, 0);
        }
      } else if (!state.reading && !state.ended) {
        state.reading = true;
        if (typeof process !== "undefined" && typeof process.nextTick === "function") {
          process.nextTick(() => {
            if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
          });
        } else {
          setTimeout(() => {
            if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
          }, 0);
        }
      }
    }
    return this;
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
    let res: any;
    if (size === undefined || size >= state.length) {
      const chunks = state.buffer;
      state.buffer = [];
      state.length = 0;
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
    } else {
      // Sliced read
      const chunk = state.buffer.shift();
      state.length -= (chunk && chunk.length) || 0;
      res = chunk;
    }
    if (!state.ended && state.length < state.highWaterMark && this.listeners("readable").length > 0) {
      state.reading = true;
      if (typeof process !== "undefined" && typeof process.nextTick === "function") {
        process.nextTick(() => {
          if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
        });
      } else {
        setTimeout(() => {
          if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
        }, 0);
      }
    }
    return res;
  }

  push(chunk: any, encoding?: string): boolean {
    const state = this._readableState;
    if (chunk === null) {
      state.ended = true;
      state.reading = false;
      if (state.length === 0 && !state.endEmitted) {
        state.endEmitted = true;
        this.readableEnded = true;
        this.emit("end");
      }
      this.emit("readable");
      return false;
    }
    if (state.objectMode || (typeof chunk === "string" && state.encoding)) {
      // An object-mode stream carries values as they are; a string stays a string under an
      // encoding.
    } else if (typeof chunk === "string" && typeof (globalThis as any).Buffer !== "undefined") {
      chunk = (globalThis as any).Buffer.from(chunk, encoding);
    }
    if (chunk === "" || (chunk && typeof chunk.length === "number" && chunk.length === 0)) {
      if (!state.ended && this.listeners("readable").length > 0) {
        state.reading = true;
        if (typeof process !== "undefined" && typeof process.nextTick === "function") {
          process.nextTick(() => {
            if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
          });
        } else {
          setTimeout(() => {
            if (!this.destroyed && !state.ended) this._read(state.highWaterMark);
          }, 0);
        }
      }
      return state.length < state.highWaterMark;
    }
    if (state.flowing && state.buffer.length === 0) {
      // Flowing: the chunk goes straight to the listeners and is not buffered, so `push(null)`
      // afterwards finds nothing pending and ends the stream. The source is asked again on the
      // next tick, the way node's `maybeReadMore` does, unless it is already reading.
      state.reading = false;
      this.emit("data", chunk);
      if (!state.ended && state.flowing && !state.reading) {
        state.reading = true;
        const again = () => {
          state.reading = false;
          if (this.destroyed || state.ended || !state.flowing) return;
          state.reading = true;
          this._read(state.highWaterMark);
        };
        if (typeof process !== "undefined" && typeof process.nextTick === "function") process.nextTick(again);
        else setTimeout(again, 0);
      }
      return true;
    }
    state.buffer.push(chunk);
    state.length += (chunk && chunk.length) || 0;
    state.reading = state.length < state.highWaterMark;
    this.emit("data", chunk);
    this.emit("readable");
    return state.length < state.highWaterMark;
  }

  pause(): this {
    this._readableState.flowing = false;
    this.emit("pause");
    return this;
  }

  resume(): this {
    const state = this._readableState;
    state.flowing = true;
    this.emit("resume");
    // Buffered chunks go out first, then the source is asked for more.
    const schedule = (fn: () => void) => {
      if (typeof process !== "undefined" && typeof process.nextTick === "function") process.nextTick(fn);
      else setTimeout(fn, 0);
    };
    schedule(() => {
      if (this.destroyed || !state.flowing) return;
      while (state.flowing && state.buffer.length) {
        const chunk = state.buffer.shift();
        state.length -= (chunk && chunk.length) || 0;
        this.emit("data", chunk);
      }
      if (state.ended) {
        if (!state.endEmitted && state.buffer.length === 0) {
          state.endEmitted = true;
          this.readableEnded = true;
          this.emit("end");
        }
        return;
      }
      if (!state.reading) {
        state.reading = true;
        this._read(state.highWaterMark);
      }
    });
    return this;
  }

  setEncoding(enc: string): this {
    this._readableState.encoding = enc;
    return this;
  }

  // `for await (const chunk of readable)`: node's Readable is async-iterable, delivering each
  // `data` chunk in order and finishing on `end`; an `error` rejects the pending read, and a
  // consumer that returns early destroys the stream.
  [Symbol.asyncIterator](): AsyncIterableIterator<any> {
    const stream: any = this;
    const queue: any[] = [];
    const waiters: Array<[(v: any) => void, (e: any) => void]> = [];
    let ended = false;
    let failure: any = null;
    const settle = () => {
      while (waiters.length && (queue.length || ended || failure)) {
        const [resolve, reject] = waiters.shift()!;
        if (queue.length) resolve({ value: queue.shift(), done: false });
        else if (failure) { const e = failure; failure = null; reject(e); }
        else resolve({ value: undefined, done: true });
      }
    };
    stream.on("data", (chunk: any) => { queue.push(chunk); settle(); });
    stream.on("end", () => { ended = true; settle(); });
    stream.on("error", (err: any) => { failure = err; settle(); });
    if (typeof stream.resume === "function") stream.resume();
    const iterator: any = {
      next() {
        return new Promise((resolve, reject) => { waiters.push([resolve, reject]); settle(); });
      },
      return() {
        ended = true;
        if (typeof stream.destroy === "function") stream.destroy();
        settle();
        return Promise.resolve({ value: undefined, done: true });
      },
      throw(err: any) { ended = true; if (typeof stream.destroy === "function") stream.destroy(err); return Promise.reject(err); },
      [Symbol.asyncIterator]() { return iterator; },
    };
    return iterator;
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
    if (options && typeof options.flush === "function") {
      (this as any)._flush = options.flush;
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

  // Ending the writable side flushes and then ends the readable side: without the `push(null)`
  // a `Transform` in a `pipe` chain never ended, and nothing downstream ever finished.
  end(chunk?: any, encoding?: any, callback?: any): this {
    if (typeof chunk === "function") {
      callback = chunk;
      chunk = undefined;
    } else if (typeof encoding === "function") {
      callback = encoding;
      encoding = undefined;
    }
    if (chunk !== undefined && chunk !== null) this.write(chunk, encoding);
    const state = this._writableState;
    if (state.ended) {
      if (callback) callback();
      return this;
    }
    state.ended = true;
    this.writableEnded = true;
    const done = (err?: any, data?: any) => {
      if (err) {
        this.emit("error", err);
        if (callback) callback(err);
        return;
      }
      if (data !== undefined && data !== null) this.push(data);
      this.push(null);
      state.finished = true;
      this.writableFinished = true;
      this.emit("finish");
      if (callback) callback();
    };
    const flush = (this as any)._flush;
    if (typeof flush === "function") flush.call(this, done);
    else done();
    return this;
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

export const promises = {
  finished(stream: any, options?: any): Promise<void> {
    return new Promise((resolve, reject) => {
      finished(stream, (err?: any) => {
        if (err) reject(err);
        else resolve();
      });
    });
  },
  pipeline(...streams: any[]): Promise<void> {
    return new Promise((resolve, reject) => {
      pipeline(...streams, (err?: any) => {
        if (err) reject(err);
        else resolve();
      });
    });
  },
};

// `Readable.from(iterable[, options])`: a readable that pulls from an iterable or async
// iterable, in object mode unless told otherwise (a string or Buffer is one chunk, as in node).
(Readable as any).from = function from(iterable: any, options?: any): Readable {
  const opts = Object.assign({ objectMode: true }, options || {});
  if (typeof iterable === "string" || (typeof Buffer !== "undefined" && Buffer.isBuffer(iterable))) {
    const single = new Readable(Object.assign({}, opts, { read() { this.push(iterable); this.push(null); } }));
    return single;
  }
  let iterator: any = null;
  let reading = false;
  const readable = new Readable(Object.assign({}, opts, {
    read() {
      if (reading) return;
      reading = true;
      if (iterator === null) {
        if (iterable && typeof iterable[Symbol.asyncIterator] === "function") iterator = iterable[Symbol.asyncIterator]();
        else if (iterable && typeof iterable[Symbol.iterator] === "function") iterator = iterable[Symbol.iterator]();
        else throw new TypeError("The \"iterable\" argument must be an instance of Iterable or AsyncIterable");
      }
      const step = () => {
        let result: any;
        try { result = iterator.next(); } catch (err) { readable.destroy(err); return; }
        Promise.resolve(result).then((r: any) => {
          if (r.done) { reading = false; readable.push(null); return; }
          Promise.resolve(r.value).then((value: any) => {
            reading = false;
            if (readable.push(value)) { reading = true; step(); }
          }, (err: any) => readable.destroy(err));
        }, (err: any) => readable.destroy(err));
      };
      step();
    },
  }));
  return readable;
};

Stream.Readable = Readable;
Stream.Writable = Writable;
Stream.Duplex = Duplex;
Stream.Transform = Transform;
Stream.PassThrough = PassThrough;
Stream.pipeline = pipeline;
Stream.finished = finished;
Stream.promises = promises;
Stream.addAbortSignal = addAbortSignal;
Stream.Stream = Stream;
Stream.default = Stream;

export default Stream;
