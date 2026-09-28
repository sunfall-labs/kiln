// kiln-builtins: node:events implementation (Phase 5.2, ADR 0005)

import { AsyncResource } from "node:async_hooks";

export class EventEmitter {
  static defaultMaxListeners: number = 10;
  static captureRejectionSymbol: symbol = Symbol.for("nodejs.rejection");
  static captureRejections: boolean = false;
  static errorMonitor: symbol = Symbol("events.errorMonitor");

  static EventEmitter: typeof EventEmitter;
  static default: typeof EventEmitter;

  static listenerCount(emitter: any, type: string | symbol): number {
    if (typeof emitter.listenerCount === "function") {
      return emitter.listenerCount(type);
    }
    return EventEmitter.prototype.listenerCount.call(emitter, type);
  }

  static getEventListeners(emitter: any, type: string | symbol): Function[] {
    if (typeof emitter.listeners === "function") {
      return emitter.listeners(type);
    }
    return EventEmitter.prototype.listeners.call(emitter, type);
  }

  static setMaxListeners(n: number, ...eventTargets: any[]): void {
    const num = Number(n);
    if (isNaN(num) || num < 0) {
      throw new RangeError("The value of \"n\" is out of range. It must be >= 0.");
    }
    for (let i = 0; i < eventTargets.length; i++) {
      eventTargets[i].setMaxListeners(num);
    }
  }

  static getMaxListeners(emitter: any): number {
    if (typeof emitter.getMaxListeners === "function") {
      return emitter.getMaxListeners();
    }
    return EventEmitter.prototype.getMaxListeners.call(emitter);
  }

  static init(this: any): void {
    if (
      this._events === undefined ||
      this._events === Object.getPrototypeOf(this)?._events
    ) {
      this._events = Object.create(null);
      this._eventsCount = 0;
    }
    this._maxListeners = this._maxListeners || undefined;
  }

  _events: any;
  _eventsCount: number;
  _maxListeners: number | undefined;

  constructor() {
    this._events = Object.create(null);
    this._eventsCount = 0;
    this._maxListeners = undefined;
  }

  addListener(type: string | symbol, listener: any): this {
    return _addListener(this, type, listener, false);
  }

  on(type: string | symbol, listener: any): this {
    return _addListener(this, type, listener, false);
  }

  prependListener(type: string | symbol, listener: any): this {
    return _addListener(this, type, listener, true);
  }

  once(type: string | symbol, listener: any): this {
    _checkListener(listener);
    this.addListener(type, _onceWrap(this, type, listener));
    return this;
  }

  prependOnceListener(type: string | symbol, listener: any): this {
    _checkListener(listener);
    this.prependListener(type, _onceWrap(this, type, listener));
    return this;
  }

  removeListener(type: string | symbol, listener: any): this {
    _checkListener(listener);
    const events = this._events;
    if (!events) return this;
    const list = events[type];
    if (!list) return this;

    if (list === listener || list.listener === listener) {
      if (--this._eventsCount === 0) {
        this._events = Object.create(null);
      } else {
        delete events[type];
        if (events.removeListener) {
          this.emit("removeListener", type, list.listener || list);
        }
      }
    } else if (Array.isArray(list)) {
      let position = -1;
      for (let i = list.length - 1; i >= 0; i--) {
        if (list[i] === listener || list[i].listener === listener) {
          position = i;
          break;
        }
      }
      if (position < 0) return this;

      if (position === 0) {
        list.shift();
      } else {
        list.splice(position, 1);
      }

      if (list.length === 1) {
        events[type] = list[0];
      }

      if (events.removeListener) {
        this.emit("removeListener", type, listener);
      }
    }

    return this;
  }

  off(type: string | symbol, listener: any): this {
    return this.removeListener(type, listener);
  }

  removeAllListeners(type?: string | symbol): this {
    const events = this._events;
    if (!events) return this;

    // Emit removeListener for all listeners if listener exists
    if (!events.removeListener) {
      if (arguments.length === 0) {
        this._events = Object.create(null);
        this._eventsCount = 0;
      } else if (events[type as any] !== undefined) {
        if (--this._eventsCount === 0) {
          this._events = Object.create(null);
        } else {
          delete events[type as any];
        }
      }
      return this;
    }

    // Emit removeListener for all registered listeners
    if (arguments.length === 0) {
      const keys = this.eventNames();
      for (let i = 0; i < keys.length; i++) {
        const key = keys[i];
        if (key === "removeListener") continue;
        this.removeAllListeners(key);
      }
      this.removeAllListeners("removeListener");
      this._events = Object.create(null);
      this._eventsCount = 0;
      return this;
    }

    const list = events[type as any];
    if (typeof list === "function") {
      this.removeListener(type as any, list);
    } else if (Array.isArray(list)) {
      for (let i = list.length - 1; i >= 0; i--) {
        this.removeListener(type as any, list[i]);
      }
    }

    return this;
  }

  setMaxListeners(n: number): this {
    const num = Number(n);
    if (isNaN(num) || num < 0) {
      throw new RangeError("The value of \"n\" is out of range. It must be >= 0.");
    }
    this._maxListeners = num;
    return this;
  }

  getMaxListeners(): number {
    if (this._maxListeners === undefined) {
      return EventEmitter.defaultMaxListeners;
    }
    return this._maxListeners;
  }

  listeners(type: string | symbol): Function[] {
    return _listeners(this, type, true);
  }

  rawListeners(type: string | symbol): Function[] {
    return _listeners(this, type, false);
  }

  emit(type: string | symbol, ...args: any[]): boolean {
    const events = this._events;
    if (events !== undefined) {
      if (type === "error" && events[EventEmitter.errorMonitor] !== undefined) {
        this.emit(EventEmitter.errorMonitor, ...args);
      }
    }

    if (type === "error") {
      let doError = true;
      if (events !== undefined) {
        if (events.error !== undefined) {
          doError = false;
        }
      }
      if (doError) {
        const err = args[0];
        if (err instanceof Error) {
          throw err;
        }
        const error = new Error(`Unhandled error.${err ? ` (${err})` : ""}`);
        (error as any).context = err;
        throw error;
      }
    }

    const handler = events ? events[type] : undefined;
    if (handler === undefined) {
      return false;
    }

    if (typeof handler === "function") {
      handler.apply(this, args);
    } else if (Array.isArray(handler)) {
      const copy = handler.slice();
      for (let i = 0; i < copy.length; i++) {
        copy[i].apply(this, args);
      }
    }

    return true;
  }

  listenerCount(type: string | symbol): number {
    const events = this._events;
    if (events !== undefined) {
      const handler = events[type];
      if (typeof handler === "function") {
        return 1;
      }
      if (Array.isArray(handler)) {
        return handler.length;
      }
    }
    return 0;
  }

  eventNames(): (string | symbol)[] {
    if (this._eventsCount === 0 || !this._events) {
      return [];
    }
    const keys = Object.keys(this._events);
    const symbols = typeof Object.getOwnPropertySymbols === "function"
      ? Object.getOwnPropertySymbols(this._events)
      : [];
    const all: (string | symbol)[] = (keys as (string | symbol)[]).concat(symbols);
    return all.filter((key) => this._events[key] !== undefined);
  }
}

// The emitter owns an AsyncResource so listeners run in the context in which the
// emitter was created. Piscina also passes this resource's asyncId to each task.
export class EventEmitterAsyncResource extends EventEmitter {
  #resource: any;

  constructor(options: any = {}) {
    super();
    if (options === null || typeof options !== "object") {
      throw new TypeError("The \"options\" argument must be of type object");
    }
    const name = options.name === undefined ? new.target.name : options.name;
    this.#resource = new AsyncResource(name, options);
    this.#resource.eventEmitter = this;
  }

  emit(type: string | symbol, ...args: any[]): boolean {
    return this.#resource.runInAsyncScope(EventEmitter.prototype.emit, this, type, ...args);
  }

  emitDestroy(): void {
    this.#resource.emitDestroy();
  }

  get asyncId(): number { return this.#resource.asyncId(); }
  get triggerAsyncId(): number { return this.#resource.triggerAsyncId(); }
  get asyncResource(): any { return this.#resource; }
}

// Aliases
EventEmitter.EventEmitter = EventEmitter;
EventEmitter.default = EventEmitter;
(EventEmitter as any).EventEmitterAsyncResource = EventEmitterAsyncResource;

// Ensure addListener and on reference the exact same function object
EventEmitter.prototype.on = EventEmitter.prototype.addListener;
EventEmitter.prototype.off = EventEmitter.prototype.removeListener;

function _checkListener(listener: any): void {
  if (typeof listener !== "function") {
    throw new TypeError(
      `The "listener" argument must be of type function. Received ${typeof listener}`
    );
  }
}

function _addListener(
  emitter: any,
  type: string | symbol,
  listener: any,
  prepend: boolean
): any {
  _checkListener(listener);

  let events = emitter._events;
  if (!events) {
    events = emitter._events = Object.create(null);
    emitter._eventsCount = 0;
  } else if (events.newListener !== undefined) {
    emitter.emit("newListener", type, listener.listener || listener);
    // Re-read in case newListener listener modified _events
    events = emitter._events;
  }

  let existing = events[type];
  if (existing === undefined) {
    events[type] = listener;
    emitter._eventsCount++;
  } else if (typeof existing === "function") {
    events[type] = prepend ? [listener, existing] : [existing, listener];
  } else if (prepend) {
    existing.unshift(listener);
  } else {
    existing.push(listener);
  }

  return emitter;
}

function _onceWrap(target: any, type: string | symbol, listener: any): any {
  let fired = false;
  function onceWrapper(this: any, ...args: any[]) {
    if (!fired) {
      fired = true;
      target.removeListener(type, onceWrapper);
      listener.apply(this, args);
    }
  }
  (onceWrapper as any).listener = listener;
  return onceWrapper;
}

function _listeners(
  emitter: any,
  type: string | symbol,
  unwrap: boolean
): Function[] {
  const events = emitter._events;
  if (!events) return [];
  const list = events[type];
  if (!list) return [];
  if (typeof list === "function") {
    return [unwrap ? list.listener || list : list];
  }
  const copy = new Array(list.length);
  for (let i = 0; i < list.length; i++) {
    copy[i] = unwrap ? list[i].listener || list[i] : list[i];
  }
  return copy;
}

// `events.once(emitter, name[, { signal }])`: a promise for the next emission's arguments,
// rejected by the first `error` in the meantime (unless `error` is what is awaited) or by the
// signal's abort. Written here rather than in either host because it is nothing but the
// emitter's own `on`/`off`.
export function once(emitter: any, name: any, options?: any): Promise<any[]> {
  const signal = options && options.signal;
  return new Promise((resolve, reject) => {
    if (signal && signal.aborted) {
      reject(abortError(signal));
      return;
    }
    const cleanup = () => {
      emitter.removeListener(name, onEvent);
      if (name !== "error") emitter.removeListener("error", onError);
      if (signal) signal.removeEventListener("abort", onAbort);
    };
    const onEvent = (...args: any[]) => { cleanup(); resolve(args); };
    const onError = (err: any) => { cleanup(); reject(err); };
    const onAbort = () => { cleanup(); reject(abortError(signal)); };
    emitter.on(name, onEvent);
    if (name !== "error") emitter.on("error", onError);
    if (signal) signal.addEventListener("abort", onAbort, { once: true });
  });
}

function abortError(signal: any): any {
  const err: any = new Error("The operation was aborted");
  err.name = "AbortError";
  err.code = "ABORT_ERR";
  err.cause = signal.reason;
  return err;
}

// `events.on(emitter, name)`: an async iterator over emissions, ended by `error` or a close.
export function on(emitter: any, name: any, options?: any): AsyncIterableIterator<any[]> {
  const queue: any[][] = [];
  const waiters: Array<[(v: any) => void, (e: any) => void]> = [];
  let error: any = null;
  let done = false;
  const onEvent = (...args: any[]) => {
    const waiter = waiters.shift();
    if (waiter) waiter[0]({ value: args, done: false });
    else queue.push(args);
  };
  const onError = (err: any) => {
    error = err;
    const waiter = waiters.shift();
    if (waiter) waiter[1](err);
  };
  emitter.on(name, onEvent);
  if (name !== "error") emitter.on("error", onError);
  const finish = () => {
    if (done) return;
    done = true;
    emitter.removeListener(name, onEvent);
    if (name !== "error") emitter.removeListener("error", onError);
    for (const [resolve] of waiters.splice(0)) resolve({ value: undefined, done: true });
  };
  const iterator: any = {
    next() {
      if (queue.length) return Promise.resolve({ value: queue.shift(), done: false });
      if (error) { const e = error; error = null; return Promise.reject(e); }
      if (done) return Promise.resolve({ value: undefined, done: true });
      return new Promise((resolve, reject) => waiters.push([resolve, reject]));
    },
    return() { finish(); return Promise.resolve({ value: undefined, done: true }); },
    throw(err: any) { finish(); return Promise.reject(err); },
    [Symbol.asyncIterator]() { return iterator; },
  };
  if (options && options.signal) {
    options.signal.addEventListener("abort", () => { error = abortError(options.signal); finish(); }, { once: true });
  }
  return iterator;
}

export const getEventListeners = (emitter: any, name: any): Function[] =>
  typeof emitter.listeners === "function" ? emitter.listeners(name) : [];

(EventEmitter as any).once = once;
(EventEmitter as any).on = on;
(EventEmitter as any).getEventListeners = getEventListeners;

export default EventEmitter;
