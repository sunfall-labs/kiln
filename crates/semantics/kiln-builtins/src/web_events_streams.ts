// kiln-builtins: the WHATWG event and stream classes node puts on every global object
// (ADR 0005, ADR 0063 rule 4): Event, CustomEvent, EventTarget, MessageChannel, MessagePort,
// BroadcastChannel, the two queuing strategies, and the three streams with their controllers,
// reader and writer.
//
// Until 2026-09-19 the interpreter published eleven of these as hollow stubs -- `new Event("x")`
// had no `type`, an `EventTarget` had no `addEventListener` -- and the native backend published
// none. A stub that answers `undefined` is the class of failure ADR 0042 forbids, so this file
// replaces them in both engines; it runs after the fetch classes in the Node-mode bootstrap.
//
// The streams are the default (non-byte) streams with the observable surface a program uses:
// controllers with backpressure through `desiredSize`, readers and writers with their locks,
// `pipeTo`/`pipeThrough`/`tee`, async iteration and `ReadableStream.from`. A `type: "bytes"`
// source is treated as a default stream whose chunks are the views it enqueues.

(function installWebEventsAndStreams() {
  const G = globalThis as any;

  function invalidState(message: string): Error {
    const DOMExc = G.DOMException;
    if (typeof DOMExc === "function") return new DOMExc(message, "InvalidStateError");
    const error: any = new Error(message);
    error.name = "InvalidStateError";
    return error;
  }

  // The `_`-prefixed fields are internal slots: node's objects have no own enumerable
  // properties, and `Object.keys(new Event("x"))` is `[]` there.
  function hideSlots(target: any): void {
    for (const key of Object.keys(target)) {
      if (key.charCodeAt(0) === 95 /* _ */) {
        Object.defineProperty(target, key, { value: target[key], writable: true, enumerable: false, configurable: true });
      }
    }
  }

  function abortError(reason?: any): Error {
    if (reason !== undefined) return reason;
    const DOMExc = G.DOMException;
    if (typeof DOMExc === "function") return new DOMExc("The operation was aborted", "AbortError");
    const error: any = new Error("The operation was aborted");
    error.name = "AbortError";
    return error;
  }

  // ---- Event, CustomEvent, EventTarget ------------------------------------------------------

  class Event {
    _type: string;
    _bubbles: boolean;
    _cancelable: boolean;
    _composed: boolean;
    _defaultPrevented = false;
    _target: any = null;
    _currentTarget: any = null;
    _eventPhase = 0;
    _stop = false;
    _stopImmediate = false;
    _timeStamp: number;
    _dispatching = false;

    static NONE = 0;
    static CAPTURING_PHASE = 1;
    static AT_TARGET = 2;
    static BUBBLING_PHASE = 3;

    constructor(type: any, init?: any) {
      if (arguments.length === 0) throw new TypeError("The \"type\" argument must be specified");
      this._type = String(type);
      const options = init === undefined || init === null ? {} : init;
      this._bubbles = !!options.bubbles;
      this._cancelable = !!options.cancelable;
      this._composed = !!options.composed;
      this._timeStamp = typeof G.performance?.now === "function" ? G.performance.now() : Date.now();
      hideSlots(this);
    }

    get type(): string { return this._type; }
    get bubbles(): boolean { return this._bubbles; }
    get cancelable(): boolean { return this._cancelable; }
    get composed(): boolean { return this._composed; }
    get defaultPrevented(): boolean { return this._defaultPrevented; }
    get target(): any { return this._target; }
    get srcElement(): any { return this._target; }
    get currentTarget(): any { return this._currentTarget; }
    get eventPhase(): number { return this._eventPhase; }
    get timeStamp(): number { return this._timeStamp; }
    get isTrusted(): boolean { return false; }
    get returnValue(): boolean { return !this._defaultPrevented; }
    set returnValue(value: boolean) { if (!value) this.preventDefault(); }
    get cancelBubble(): boolean { return this._stop; }
    set cancelBubble(value: boolean) { if (value) this._stop = true; }

    preventDefault(): void {
      if (this._cancelable) this._defaultPrevented = true;
    }
    stopPropagation(): void { this._stop = true; }
    stopImmediatePropagation(): void { this._stop = true; this._stopImmediate = true; }
    composedPath(): any[] { return this._currentTarget ? [this._currentTarget] : []; }
    initEvent(type: any, bubbles?: any, cancelable?: any): void {
      if (this._dispatching) return;
      this._type = String(type);
      this._bubbles = !!bubbles;
      this._cancelable = !!cancelable;
    }
    get [Symbol.toStringTag](): string { return "Event"; }
  }
  for (const [name, value] of [["NONE", 0], ["CAPTURING_PHASE", 1], ["AT_TARGET", 2], ["BUBBLING_PHASE", 3]] as [string, number][]) {
    Object.defineProperty(Event.prototype, name, { value, writable: false, enumerable: true, configurable: false });
  }

  class CustomEvent extends Event {
    _detail: any;
    constructor(type: any, init?: any) {
      super(type, init);
      this._detail = init && init.detail !== undefined ? init.detail : null;
      hideSlots(this);
    }
    get detail(): any { return this._detail; }
    get [Symbol.toStringTag](): string { return "CustomEvent"; }
  }

  interface Listener {
    callback: any;
    capture: boolean;
    once: boolean;
    passive: boolean;
    signal: any;
    removed: boolean;
  }

  const listenersOf = new WeakMap<object, Map<string, Listener[]>>();

  function listenerTable(target: object): Map<string, Listener[]> {
    let table = listenersOf.get(target);
    if (!table) {
      table = new Map();
      listenersOf.set(target, table);
    }
    return table;
  }

  function optionsOf(options: any): { capture: boolean; once: boolean; passive: boolean; signal: any } {
    if (typeof options === "boolean") return { capture: options, once: false, passive: false, signal: undefined };
    if (options === null || typeof options !== "object") return { capture: false, once: false, passive: false, signal: undefined };
    return { capture: !!options.capture, once: !!options.once, passive: !!options.passive, signal: options.signal };
  }

  class EventTarget {
    addEventListener(type: any, callback: any, options?: any): void {
      if (callback === null || callback === undefined) return;
      const parsed = optionsOf(options);
      if (parsed.signal !== undefined && parsed.signal !== null && parsed.signal.aborted) return;
      const key = String(type);
      const table = listenerTable(this);
      let list = table.get(key);
      if (!list) {
        list = [];
        table.set(key, list);
      }
      for (const existing of list) {
        if (existing.callback === callback && existing.capture === parsed.capture) return;
      }
      const listener: Listener = { callback, capture: parsed.capture, once: parsed.once, passive: parsed.passive, signal: parsed.signal, removed: false };
      list.push(listener);
      if (parsed.signal !== undefined && parsed.signal !== null && typeof parsed.signal.addEventListener === "function") {
        parsed.signal.addEventListener("abort", () => this.removeEventListener(key, callback, { capture: parsed.capture }), { once: true });
      }
    }

    removeEventListener(type: any, callback: any, options?: any): void {
      const parsed = optionsOf(options);
      const list = listenersOf.get(this)?.get(String(type));
      if (!list) return;
      for (let i = 0; i < list.length; i++) {
        if (list[i].callback === callback && list[i].capture === parsed.capture) {
          list[i].removed = true;
          list.splice(i, 1);
          return;
        }
      }
    }

    dispatchEvent(event: any): boolean {
      if (!(event instanceof Event)) {
        throw new TypeError("The \"event\" argument must be an instance of Event. Received " + typeof event);
      }
      if (event._dispatching) throw invalidState("The event is already being dispatched");
      event._dispatching = true;
      event._target = this;
      event._currentTarget = this;
      event._eventPhase = Event.AT_TARGET;
      const list = listenersOf.get(this)?.get(event._type);
      if (list) {
        for (const listener of list.slice()) {
          if (listener.removed) continue;
          if (listener.once) this.removeEventListener(event._type, listener.callback, { capture: listener.capture });
          const callback = listener.callback;
          try {
            if (typeof callback === "function") callback.call(this, event);
            else if (callback && typeof callback.handleEvent === "function") callback.handleEvent(event);
          } catch (error) {
            // A throwing listener does not stop the others; node reports it as uncaught.
            reportUncaught(error);
          }
          if (event._stopImmediate) break;
        }
      }
      event._dispatching = false;
      event._eventPhase = Event.NONE;
      event._currentTarget = null;
      return !event._defaultPrevented;
    }
    get [Symbol.toStringTag](): string { return "EventTarget"; }
  }

  function reportUncaught(error: any): void {
    const proc = G.process;
    if (proc && typeof proc.emit === "function" && proc.listenerCount && proc.listenerCount("uncaughtException") > 0) {
      proc.emit("uncaughtException", error);
      return;
    }
    if (typeof G.queueMicrotask === "function") {
      G.queueMicrotask(() => { throw error; });
    } else {
      throw error;
    }
  }

  // ---- MessageChannel, MessagePort, BroadcastChannel ---------------------------------------

  function deliverLater(fn: () => void): void {
    if (typeof G.setImmediate === "function") G.setImmediate(fn);
    else if (typeof G.setTimeout === "function") G.setTimeout(fn, 0);
    else G.queueMicrotask(fn);
  }

  function cloneMessage(message: any): any {
    return typeof G.structuredClone === "function" ? G.structuredClone(message) : message;
  }

  class MessagePort extends EventTarget {
    _other: MessagePort | null = null;
    _started = false;
    _closed = false;
    _queue: any[] = [];
    _onmessage: any = null;
    _onmessageerror: any = null;

    constructor() {
      super();
      hideSlots(this);
    }

    get onmessage(): any { return this._onmessage; }
    set onmessage(handler: any) {
      this._onmessage = typeof handler === "function" ? handler : null;
      this.start();
    }
    get onmessageerror(): any { return this._onmessageerror; }
    set onmessageerror(handler: any) { this._onmessageerror = typeof handler === "function" ? handler : null; }

    postMessage(message: any): void {
      if (this._closed || !this._other) return;
      const other = this._other;
      const data = cloneMessage(message);
      other._queue.push(data);
      if (other._started) other._drain();
    }

    start(): void {
      if (this._started) return;
      this._started = true;
      this._drain();
    }

    _drain(): void {
      if (this._queue.length === 0) return;
      const pending = this._queue;
      this._queue = [];
      deliverLater(() => {
        for (const data of pending) {
          if (this._closed) return;
          const event = new MessageEvent("message", { data, ports: [] });
          if (this._onmessage) this._onmessage.call(this, event);
          this.dispatchEvent(event);
        }
      });
    }

    close(): void {
      if (this._closed) return;
      this._closed = true;
      const other = this._other;
      this._other = null;
      if (other && !other._closed) {
        other._closed = true;
        other._other = null;
        deliverLater(() => other.dispatchEvent(new Event("close")));
      }
      deliverLater(() => this.dispatchEvent(new Event("close")));
    }
    ref(): this { return this; }
    unref(): this { return this; }
    hasRef(): boolean { return this._started && !this._closed; }
    get [Symbol.toStringTag](): string { return "MessagePort"; }
  }

  class MessageEvent extends Event {
    _data: any;
    _origin: string;
    _lastEventId: string;
    _source: any;
    _ports: any[];
    constructor(type: any, init?: any) {
      super(type, init);
      const options = init || {};
      this._data = options.data === undefined ? null : options.data;
      this._origin = options.origin === undefined ? "" : String(options.origin);
      this._lastEventId = options.lastEventId === undefined ? "" : String(options.lastEventId);
      this._source = options.source === undefined ? null : options.source;
      this._ports = options.ports ? Array.from(options.ports) : [];
      hideSlots(this);
    }
    get data(): any { return this._data; }
    get origin(): string { return this._origin; }
    get lastEventId(): string { return this._lastEventId; }
    get source(): any { return this._source; }
    get ports(): any[] { return this._ports; }
    get [Symbol.toStringTag](): string { return "MessageEvent"; }
  }

  class MessageChannel {
    port1: MessagePort;
    port2: MessagePort;
    constructor() {
      this.port1 = new MessagePort();
      this.port2 = new MessagePort();
      this.port1._other = this.port2;
      this.port2._other = this.port1;
    }
    get [Symbol.toStringTag](): string { return "MessageChannel"; }
  }

  const broadcastChannels = new Map<string, Set<BroadcastChannel>>();

  class BroadcastChannel extends EventTarget {
    _name: string;
    _closed = false;
    _onmessage: any = null;
    _onmessageerror: any = null;
    constructor(name: any) {
      super();
      if (arguments.length === 0) throw new TypeError("The \"name\" argument must be specified");
      this._name = String(name);
      let set = broadcastChannels.get(this._name);
      if (!set) {
        set = new Set();
        broadcastChannels.set(this._name, set);
      }
      set.add(this);
      hideSlots(this);
    }
    get name(): string { return this._name; }
    get onmessage(): any { return this._onmessage; }
    set onmessage(handler: any) { this._onmessage = typeof handler === "function" ? handler : null; }
    get onmessageerror(): any { return this._onmessageerror; }
    set onmessageerror(handler: any) { this._onmessageerror = typeof handler === "function" ? handler : null; }
    postMessage(message: any): void {
      if (this._closed) throw invalidState("BroadcastChannel is closed");
      const data = cloneMessage(message);
      const peers = broadcastChannels.get(this._name);
      if (!peers) return;
      for (const peer of peers) {
        if (peer === this || peer._closed) continue;
        deliverLater(() => {
          if (peer._closed) return;
          const event = new MessageEvent("message", { data });
          if (peer._onmessage) peer._onmessage.call(peer, event);
          peer.dispatchEvent(event);
        });
      }
    }
    close(): void {
      if (this._closed) return;
      this._closed = true;
      broadcastChannels.get(this._name)?.delete(this);
    }
    ref(): this { return this; }
    unref(): this { return this; }
    get [Symbol.toStringTag](): string { return "BroadcastChannel"; }
  }

  // ---- Queuing strategies --------------------------------------------------------------------

  class ByteLengthQueuingStrategy {
    highWaterMark: number;
    constructor(init: any) {
      if (init === null || typeof init !== "object" || !("highWaterMark" in init)) {
        throw new TypeError("The \"init.highWaterMark\" property must be specified");
      }
      this.highWaterMark = Number(init.highWaterMark);
    }
    size(chunk: any): number { return chunk.byteLength; }
    get [Symbol.toStringTag](): string { return "ByteLengthQueuingStrategy"; }
  }

  class CountQueuingStrategy {
    highWaterMark: number;
    constructor(init: any) {
      if (init === null || typeof init !== "object" || !("highWaterMark" in init)) {
        throw new TypeError("The \"init.highWaterMark\" property must be specified");
      }
      this.highWaterMark = Number(init.highWaterMark);
    }
    size(): number { return 1; }
    get [Symbol.toStringTag](): string { return "CountQueuingStrategy"; }
  }

  function strategyOf(strategy: any, defaultHighWaterMark: number): { highWaterMark: number; size: (chunk: any) => number } {
    const s = strategy === undefined || strategy === null ? {} : strategy;
    let highWaterMark = s.highWaterMark === undefined ? defaultHighWaterMark : Number(s.highWaterMark);
    if (Number.isNaN(highWaterMark) || highWaterMark < 0) throw new RangeError("Invalid highWaterMark");
    const size = typeof s.size === "function" ? s.size : () => 1;
    return { highWaterMark, size };
  }

  function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void; reject: (e: any) => void } {
    let resolve!: (v: T) => void;
    let reject!: (e: any) => void;
    const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
  }

  function callOrDefault(source: any, name: string, args: any[], fallback: any): any {
    const method = source ? source[name] : undefined;
    if (typeof method === "function") return method.apply(source, args);
    if (method !== undefined && method !== null) throw new TypeError(`${name} is not a function`);
    return fallback;
  }

  // ---- ReadableStream ------------------------------------------------------------------------

  class ReadableStreamDefaultController {
    _stream: ReadableStream;
    _queue: { chunk: any; size: number }[] = [];
    _queueSize = 0;
    _strategy: { highWaterMark: number; size: (chunk: any) => number };
    _source: any;
    _closeRequested = false;
    _pulling = false;
    _pullAgain = false;
    _started = false;

    constructor(stream: ReadableStream, source: any, strategy: { highWaterMark: number; size: (chunk: any) => number }) {
      this._stream = stream;
      this._source = source;
      this._strategy = strategy;
      hideSlots(this);
    }

    get desiredSize(): number | null {
      const state = this._stream._state;
      if (state === "errored") return null;
      if (state === "closed") return 0;
      return this._strategy.highWaterMark - this._queueSize;
    }

    enqueue(chunk?: any): void {
      if (this._closeRequested || this._stream._state !== "readable") {
        throw new TypeError("Invalid state: The stream is not in a state that permits enqueue");
      }
      const stream = this._stream;
      if (stream._reader && stream._reader._requests.length > 0) {
        const request = stream._reader._requests.shift()!;
        request.resolve({ value: chunk, done: false });
      } else {
        let size = 1;
        try {
          size = Number(this._strategy.size(chunk));
          if (Number.isNaN(size) || size < 0 || size === Infinity) throw new RangeError("The return value of a queuing strategy's size function must be a finite, non-negative number");
        } catch (error) {
          this.error(error);
          throw error;
        }
        this._queue.push({ chunk, size });
        this._queueSize += size;
      }
      this._pullIfNeeded();
    }

    close(): void {
      if (this._closeRequested || this._stream._state !== "readable") {
        throw new TypeError("Invalid state: The stream is not in a state that permits close");
      }
      this._closeRequested = true;
      if (this._queue.length === 0) this._stream._close();
    }

    error(reason?: any): void {
      if (this._stream._state !== "readable") return;
      this._queue = [];
      this._queueSize = 0;
      this._stream._error(reason);
    }

    _pullIfNeeded(): void {
      if (!this._started || this._closeRequested || this._stream._state !== "readable") return;
      const reader = this._stream._reader;
      const wanted = (reader && reader._requests.length > 0) || (this.desiredSize as number) > 0;
      if (!wanted) return;
      if (this._pulling) {
        this._pullAgain = true;
        return;
      }
      this._pulling = true;
      let pulled: Promise<any>;
      try {
        pulled = Promise.resolve(callOrDefault(this._source, "pull", [this], undefined));
      } catch (error) {
        this.error(error);
        return;
      }
      pulled.then(
        () => {
          this._pulling = false;
          if (this._pullAgain) {
            this._pullAgain = false;
            this._pullIfNeeded();
          }
        },
        (error) => this.error(error),
      );
    }

    _dequeue(): any {
      const entry = this._queue.shift()!;
      this._queueSize -= entry.size;
      if (this._queueSize < 0) this._queueSize = 0;
      if (this._closeRequested && this._queue.length === 0) this._stream._close();
      else this._pullIfNeeded();
      return entry.chunk;
    }

    _cancel(reason: any): Promise<void> {
      this._queue = [];
      this._queueSize = 0;
      return Promise.resolve(callOrDefault(this._source, "cancel", [reason], undefined)).then(() => undefined);
    }
    get [Symbol.toStringTag](): string { return "ReadableStreamDefaultController"; }
  }

  class ReadableStreamDefaultReader {
    _stream: ReadableStream | null;
    _requests: { resolve: (v: any) => void; reject: (e: any) => void }[] = [];
    _closed: { promise: Promise<void>; resolve: (v: void) => void; reject: (e: any) => void };

    constructor(stream: any) {
      if (!(stream instanceof ReadableStream)) throw new TypeError("ReadableStreamDefaultReader constructor argument must be a ReadableStream");
      if (stream._reader) throw new TypeError("Invalid state: ReadableStream is locked");
      this._stream = stream;
      stream._reader = this;
      this._closed = deferred<void>();
      this._closed.promise.catch(() => {});
      if (stream._state === "closed") this._closed.resolve();
      else if (stream._state === "errored") this._closed.reject(stream._storedError);
      hideSlots(this);
    }

    get closed(): Promise<void> { return this._closed.promise; }

    read(): Promise<{ value: any; done: boolean }> {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Invalid state: The reader is not attached to a stream"));
      if (stream._state === "closed") return Promise.resolve({ value: undefined, done: true });
      if (stream._state === "errored") return Promise.reject(stream._storedError);
      const controller = stream._controller!;
      if (controller._queue.length > 0) {
        const chunk = controller._dequeue();
        return Promise.resolve({ value: chunk, done: false });
      }
      const request = deferred<{ value: any; done: boolean }>();
      this._requests.push(request);
      controller._pullIfNeeded();
      return request.promise;
    }

    releaseLock(): void {
      const stream = this._stream;
      if (!stream) return;
      const pending = this._requests;
      this._requests = [];
      for (const request of pending) request.reject(new TypeError("Releasing reader"));
      if (stream._state === "readable") this._closed.reject(new TypeError("Reader was released"));
      stream._reader = null;
      this._stream = null;
    }

    cancel(reason?: any): Promise<void> {
      if (!this._stream) return Promise.reject(new TypeError("Invalid state: The reader is not attached to a stream"));
      return this._stream._cancel(reason);
    }
    get [Symbol.toStringTag](): string { return "ReadableStreamDefaultReader"; }
  }

  class ReadableStream {
    _state: "readable" | "closed" | "errored" = "readable";
    _storedError: any = undefined;
    _reader: ReadableStreamDefaultReader | null = null;
    _controller: ReadableStreamDefaultController | null = null;
    _disturbed = false;

    constructor(source?: any, strategy?: any) {
      const underlying = source === undefined || source === null ? {} : source;
      if (typeof underlying !== "object" && typeof underlying !== "function") throw new TypeError("The \"underlyingSource\" argument must be an object");
      const parsed = strategyOf(strategy, underlying.type === "bytes" ? 0 : 1);
      const controller = new ReadableStreamDefaultController(this, underlying, parsed);
      this._controller = controller;
      hideSlots(this);
      let started: Promise<any>;
      try {
        started = Promise.resolve(callOrDefault(underlying, "start", [controller], undefined));
      } catch (error) {
        controller.error(error);
        return;
      }
      started.then(
        () => {
          controller._started = true;
          controller._pullIfNeeded();
        },
        (error) => controller.error(error),
      );
    }

    static from(iterable: any): ReadableStream {
      if (iterable === null || iterable === undefined) throw new TypeError("The \"iterable\" argument must be an iterable");
      const asyncIterator = typeof iterable[Symbol.asyncIterator] === "function" ? iterable[Symbol.asyncIterator]() : null;
      const iterator = asyncIterator || (typeof iterable[Symbol.iterator] === "function" ? iterable[Symbol.iterator]() : null);
      if (!iterator) throw new TypeError("The \"iterable\" argument must be an iterable");
      return new ReadableStream(
        {
          async pull(controller: any) {
            const result = await iterator.next();
            if (result.done) controller.close();
            else controller.enqueue(result.value);
          },
          async cancel(reason: any) {
            if (typeof iterator.return === "function") await iterator.return(reason);
          },
        },
        { highWaterMark: 0 },
      );
    }

    get locked(): boolean { return this._reader !== null; }

    getReader(options?: any): ReadableStreamDefaultReader {
      if (options && options.mode !== undefined && options.mode !== "byob") throw new TypeError("Invalid mode");
      return new ReadableStreamDefaultReader(this);
    }

    cancel(reason?: any): Promise<void> {
      if (this._reader) return Promise.reject(new TypeError("Invalid state: ReadableStream is locked"));
      return this._cancel(reason);
    }

    _cancel(reason: any): Promise<void> {
      this._disturbed = true;
      if (this._state === "closed") return Promise.resolve();
      if (this._state === "errored") return Promise.reject(this._storedError);
      const controller = this._controller!;
      this._close();
      return controller._cancel(reason);
    }

    _close(): void {
      if (this._state !== "readable") return;
      this._state = "closed";
      const reader = this._reader;
      if (reader) {
        const pending = reader._requests;
        reader._requests = [];
        for (const request of pending) request.resolve({ value: undefined, done: true });
        reader._closed.resolve();
      }
    }

    _error(reason: any): void {
      if (this._state !== "readable") return;
      this._state = "errored";
      this._storedError = reason;
      const reader = this._reader;
      if (reader) {
        const pending = reader._requests;
        reader._requests = [];
        for (const request of pending) request.reject(reason);
        reader._closed.reject(reason);
      }
    }

    pipeTo(destination: any, options?: any): Promise<void> {
      if (!(destination instanceof WritableStream)) return Promise.reject(new TypeError("The \"destination\" argument must be an instance of WritableStream"));
      if (this._reader) return Promise.reject(new TypeError("Invalid state: ReadableStream is locked"));
      if (destination._writer) return Promise.reject(new TypeError("Invalid state: WritableStream is locked"));
      const opts = options || {};
      const signal = opts.signal;
      if (signal !== undefined && signal !== null && typeof signal.aborted !== "boolean") return Promise.reject(new TypeError("Invalid signal"));
      const reader = this.getReader();
      const writer = destination.getWriter();
      const source = this;
      return new Promise<void>((resolve, reject) => {
        let finished = false;
        function finish(error?: any, aborted?: boolean): void {
          if (finished) return;
          finished = true;
          const settle = () => {
            reader.releaseLock();
            writer.releaseLock();
            if (error !== undefined || aborted) reject(error);
            else resolve();
          };
          if (aborted) {
            const actions: Promise<any>[] = [];
            if (!opts.preventAbort) actions.push(writer.abort(error));
            if (!opts.preventCancel) actions.push(reader.cancel(error));
            Promise.all(actions).then(settle, settle);
          } else {
            settle();
          }
        }
        if (signal) {
          if (signal.aborted) { finish(abortError(signal.reason), true); return; }
          signal.addEventListener("abort", () => finish(abortError(signal.reason), true), { once: true });
        }
        writer.closed.catch((error: any) => {
          if (finished) return;
          if (!opts.preventCancel) reader.cancel(error).catch(() => {});
          finished = true;
          reader.releaseLock();
          reject(error);
        });
        const step = (): void => {
          if (finished) return;
          reader.read().then(
            ({ value, done }) => {
              if (finished) return;
              if (done) {
                const closing = opts.preventClose ? Promise.resolve() : writer.close();
                closing.then(() => finish(), (error: any) => finish(error));
                return;
              }
              writer.write(value).then(step, (error: any) => {
                if (finished) return;
                finished = true;
                const cancelling = opts.preventCancel ? Promise.resolve() : reader.cancel(error);
                cancelling.then(() => { reader.releaseLock(); reject(error); }, () => { reader.releaseLock(); reject(error); });
              });
            },
            (error) => {
              if (finished) return;
              finished = true;
              const aborting = opts.preventAbort ? Promise.resolve() : writer.abort(error);
              aborting.then(() => { writer.releaseLock(); reject(error); }, () => { writer.releaseLock(); reject(error); });
            },
          );
        };
        step();
      });
    }

    pipeThrough(transform: any, options?: any): any {
      if (!transform || !(transform.readable instanceof ReadableStream) || !(transform.writable instanceof WritableStream)) {
        throw new TypeError("The \"transform\" argument must have readable and writable streams");
      }
      const piped = this.pipeTo(transform.writable, options);
      piped.catch(() => {});
      return transform.readable;
    }

    tee(): [ReadableStream, ReadableStream] {
      const reader = this.getReader();
      let reading = false;
      let cancelled1 = false;
      let cancelled2 = false;
      let closed = false;
      const controllers: any[] = [];
      const pull = (): Promise<void> => {
        if (reading || closed) return Promise.resolve();
        reading = true;
        return reader.read().then(
          ({ value, done }) => {
            reading = false;
            if (done) {
              closed = true;
              if (!cancelled1) controllers[0].close();
              if (!cancelled2) controllers[1].close();
              return;
            }
            if (!cancelled1) controllers[0].enqueue(value);
            if (!cancelled2) controllers[1].enqueue(cloneMessage(value));
          },
          (error) => {
            reading = false;
            closed = true;
            if (!cancelled1) controllers[0].error(error);
            if (!cancelled2) controllers[1].error(error);
          },
        );
      };
      const make = (index: number) => new ReadableStream({
        start(controller: any) { controllers[index] = controller; },
        pull,
        cancel(reason: any) {
          if (index === 0) cancelled1 = true; else cancelled2 = true;
          if (cancelled1 && cancelled2) return reader.cancel(reason);
          return undefined;
        },
      });
      return [make(0), make(1)];
    }

    values(options?: any): AsyncIterableIterator<any> {
      const reader = this.getReader();
      const preventCancel = !!(options && options.preventCancel);
      const iterator: any = {
        next(): Promise<any> {
          return reader.read().then((result) => {
            if (result.done) reader.releaseLock();
            return result;
          });
        },
        return(value?: any): Promise<any> {
          const done = preventCancel ? Promise.resolve() : reader.cancel(value);
          return done.then(() => { reader.releaseLock(); return { value, done: true }; });
        },
        [Symbol.asyncIterator]() { return iterator; },
      };
      return iterator;
    }

    [Symbol.asyncIterator](options?: any): AsyncIterableIterator<any> { return this.values(options); }
    get [Symbol.toStringTag](): string { return "ReadableStream"; }
  }

  // ---- WritableStream ------------------------------------------------------------------------

  class WritableStreamDefaultController {
    _stream: WritableStream;
    _sink: any;
    _abortReason: any = undefined;
    _signal: any;
    constructor(stream: WritableStream, sink: any) {
      this._stream = stream;
      this._sink = sink;
      this._signal = typeof G.AbortController === "function" ? new G.AbortController() : null;
      hideSlots(this);
    }
    get signal(): any { return this._signal ? this._signal.signal : undefined; }
    error(reason?: any): void {
      if (this._stream._state !== "writable") return;
      this._stream._error(reason);
    }
    get [Symbol.toStringTag](): string { return "WritableStreamDefaultController"; }
  }

  class WritableStreamDefaultWriter {
    _stream: WritableStream | null;
    _closed: { promise: Promise<void>; resolve: (v: void) => void; reject: (e: any) => void };
    _ready: { promise: Promise<void>; resolve: (v: void) => void; reject: (e: any) => void };

    constructor(stream: any) {
      if (!(stream instanceof WritableStream)) throw new TypeError("WritableStreamDefaultWriter constructor argument must be a WritableStream");
      if (stream._writer) throw new TypeError("Invalid state: WritableStream is locked");
      this._stream = stream;
      stream._writer = this;
      this._closed = deferred<void>();
      this._closed.promise.catch(() => {});
      this._ready = deferred<void>();
      this._ready.promise.catch(() => {});
      if (stream._state === "closed") this._closed.resolve();
      else if (stream._state === "errored") this._closed.reject(stream._storedError);
      if (stream._state === "errored") this._ready.reject(stream._storedError);
      else if (stream._backpressure()) { /* ready stays pending until the queue drains */ }
      else this._ready.resolve();
      hideSlots(this);
    }

    get closed(): Promise<void> { return this._closed.promise; }
    get ready(): Promise<void> { return this._ready.promise; }
    get desiredSize(): number | null {
      const stream = this._stream;
      if (!stream) throw new TypeError("Invalid state: The writer is not attached to a stream");
      if (stream._state === "errored") return null;
      if (stream._state === "closed" || stream._closeRequested) return 0;
      return stream._strategy.highWaterMark - stream._queueSize;
    }

    write(chunk?: any): Promise<void> {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Invalid state: The writer is not attached to a stream"));
      return stream._write(chunk);
    }
    close(): Promise<void> {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Invalid state: The writer is not attached to a stream"));
      return stream._close();
    }
    abort(reason?: any): Promise<void> {
      const stream = this._stream;
      if (!stream) return Promise.reject(new TypeError("Invalid state: The writer is not attached to a stream"));
      return stream._abort(reason);
    }
    releaseLock(): void {
      const stream = this._stream;
      if (!stream) return;
      const error = new TypeError("Writer was released and can no longer be used to monitor the stream's closedness");
      if (stream._state === "writable") {
        this._closed.reject(error);
        this._ready.reject(error);
      }
      stream._writer = null;
      this._stream = null;
    }
    get [Symbol.toStringTag](): string { return "WritableStreamDefaultWriter"; }
  }

  class WritableStream {
    _state: "writable" | "closed" | "errored" | "erroring" = "writable";
    _storedError: any = undefined;
    _writer: WritableStreamDefaultWriter | null = null;
    _controller: WritableStreamDefaultController;
    _strategy: { highWaterMark: number; size: (chunk: any) => number };
    _queue: { chunk: any; size: number; done: { resolve: (v: void) => void; reject: (e: any) => void } }[] = [];
    _queueSize = 0;
    _writing = false;
    _closeRequested = false;
    _closeDone: { resolve: (v: void) => void; reject: (e: any) => void } | null = null;
    _started = false;
    _sink: any;

    constructor(sink?: any, strategy?: any) {
      const underlying = sink === undefined || sink === null ? {} : sink;
      if (typeof underlying !== "object" && typeof underlying !== "function") throw new TypeError("The \"underlyingSink\" argument must be an object");
      if (underlying.type !== undefined) throw new RangeError("Invalid type is specified");
      this._sink = underlying;
      this._strategy = strategyOf(strategy, 1);
      this._controller = new WritableStreamDefaultController(this, underlying);
      hideSlots(this);
      let started: Promise<any>;
      try {
        started = Promise.resolve(callOrDefault(underlying, "start", [this._controller], undefined));
      } catch (error) {
        this._error(error);
        return;
      }
      started.then(
        () => {
          this._started = true;
          this._advance();
        },
        (error) => this._error(error),
      );
    }

    get locked(): boolean { return this._writer !== null; }

    getWriter(): WritableStreamDefaultWriter { return new WritableStreamDefaultWriter(this); }

    abort(reason?: any): Promise<void> {
      if (this._writer) return Promise.reject(new TypeError("Invalid state: WritableStream is locked"));
      return this._abort(reason);
    }

    close(): Promise<void> {
      if (this._writer) return Promise.reject(new TypeError("Invalid state: WritableStream is locked"));
      return this._close();
    }

    _backpressure(): boolean {
      return this._strategy.highWaterMark - this._queueSize <= 0;
    }

    _write(chunk: any): Promise<void> {
      if (this._state === "errored" || this._state === "erroring") return Promise.reject(this._storedError);
      if (this._state === "closed" || this._closeRequested) return Promise.reject(new TypeError("Invalid state: The stream is closing or closed"));
      let size = 1;
      try {
        size = Number(this._strategy.size(chunk));
        if (Number.isNaN(size) || size < 0 || size === Infinity) throw new RangeError("The return value of a queuing strategy's size function must be a finite, non-negative number");
      } catch (error) {
        this._error(error);
        return Promise.reject(error);
      }
      const done = deferred<void>();
      const hadRoom = !this._backpressure();
      this._queue.push({ chunk, size, done });
      this._queueSize += size;
      if (hadRoom && this._backpressure() && this._writer) {
        this._writer._ready = deferred<void>();
        this._writer._ready.promise.catch(() => {});
      }
      this._advance();
      return done.promise;
    }

    _advance(): void {
      if (!this._started || this._writing || this._state !== "writable") return;
      if (this._queue.length === 0) {
        if (this._closeRequested && this._closeDone) {
          this._writing = true;
          const finish = this._closeDone;
          this._closeDone = null;
          Promise.resolve()
            .then(() => callOrDefault(this._sink, "close", [], undefined))
            .then(
              () => {
                this._writing = false;
                this._state = "closed";
                finish.resolve();
                if (this._writer) this._writer._closed.resolve();
              },
              (error) => {
                this._writing = false;
                this._error(error);
                finish.reject(error);
              },
            );
        }
        return;
      }
      const entry = this._queue[0];
      this._writing = true;
      Promise.resolve()
        .then(() => callOrDefault(this._sink, "write", [entry.chunk, this._controller], undefined))
        .then(
          () => {
            this._writing = false;
            this._queue.shift();
            this._queueSize -= entry.size;
            if (this._queueSize < 0) this._queueSize = 0;
            entry.done.resolve();
            if (this._writer && !this._backpressure()) this._writer._ready.resolve();
            this._advance();
          },
          (error) => {
            this._writing = false;
            this._error(error);
          },
        );
    }

    _close(): Promise<void> {
      if (this._state === "errored" || this._state === "erroring") return Promise.reject(this._storedError);
      if (this._state === "closed" || this._closeRequested) return Promise.reject(new TypeError("Invalid state: The stream is closing or closed"));
      const done = deferred<void>();
      this._closeRequested = true;
      this._closeDone = done;
      if (this._writer) this._writer._ready.resolve();
      this._advance();
      return done.promise;
    }

    _abort(reason?: any): Promise<void> {
      if (this._state === "closed" || this._state === "errored") return Promise.resolve();
      this._controller._abortReason = reason;
      if (this._controller._signal) this._controller._signal.abort(reason);
      const error = reason === undefined ? abortError() : reason;
      this._error(error);
      return Promise.resolve(callOrDefault(this._sink, "abort", [reason], undefined)).then(() => undefined);
    }

    _error(reason: any): void {
      if (this._state === "closed" || this._state === "errored") return;
      this._state = "errored";
      this._storedError = reason;
      const pending = this._queue;
      this._queue = [];
      this._queueSize = 0;
      for (const entry of pending) entry.done.reject(reason);
      if (this._closeDone) {
        this._closeDone.reject(reason);
        this._closeDone = null;
      }
      if (this._writer) {
        this._writer._closed.reject(reason);
        this._writer._ready.reject(reason);
      }
    }
    get [Symbol.toStringTag](): string { return "WritableStream"; }
  }

  // ---- TransformStream -----------------------------------------------------------------------

  class TransformStreamDefaultController {
    _stream: TransformStream;
    constructor(stream: TransformStream) { this._stream = stream; hideSlots(this); }
    get desiredSize(): number | null { return this._stream._readableController!.desiredSize; }
    enqueue(chunk?: any): void { this._stream._readableController!.enqueue(chunk); }
    error(reason?: any): void { this._stream._readableController!.error(reason); this._stream._writable._error(reason); }
    terminate(): void {
      this._stream._readableController!.close();
      this._stream._writable._error(new TypeError("The transform stream has been terminated"));
    }
    get [Symbol.toStringTag](): string { return "TransformStreamDefaultController"; }
  }

  class TransformStream {
    readable: ReadableStream;
    writable: WritableStream;
    _writable: WritableStream;
    _readableController: ReadableStreamDefaultController | null = null;
    _controller: TransformStreamDefaultController;

    constructor(transformer?: any, writableStrategy?: any, readableStrategy?: any) {
      const t = transformer === undefined || transformer === null ? {} : transformer;
      if (t.readableType !== undefined) throw new RangeError("Invalid readableType specified");
      if (t.writableType !== undefined) throw new RangeError("Invalid writableType specified");
      this._controller = new TransformStreamDefaultController(this);
      const controller = this._controller;
      const startResult = deferred<void>();
      let startPromise: Promise<any>;
      const readableStrategyParsed = strategyOf(readableStrategy, 0);
      this.readable = new ReadableStream(
        {
          start: (readableController: any) => {
            this._readableController = readableController;
            return startResult.promise;
          },
          pull: () => undefined,
          cancel: (reason: any) => {
            this._writable._error(reason);
            return undefined;
          },
        },
        readableStrategyParsed,
      );
      const writableStrategyParsed = strategyOf(writableStrategy, 1);
      this.writable = new WritableStream(
        {
          start: () => startResult.promise,
          write: (chunk: any) => {
            const method = t.transform;
            if (typeof method === "function") return method.call(t, chunk, controller);
            controller.enqueue(chunk);
            return undefined;
          },
          close: () => {
            return Promise.resolve(typeof t.flush === "function" ? t.flush.call(t, controller) : undefined).then(() => {
              const readableController = this._readableController!;
              if (!readableController._closeRequested && this.readable._state === "readable") readableController.close();
            });
          },
          abort: (reason: any) => {
            this._readableController!.error(reason);
            return undefined;
          },
        },
        writableStrategyParsed,
      );
      this._writable = this.writable;
      hideSlots(this);
      try {
        startPromise = Promise.resolve(typeof t.start === "function" ? t.start.call(t, controller) : undefined);
      } catch (error) {
        startResult.reject(error);
        return;
      }
      startPromise.then(() => startResult.resolve(), (error) => startResult.reject(error));
    }
    get [Symbol.toStringTag](): string { return "TransformStream"; }
  }

  // ---- Install -------------------------------------------------------------------------------

  const classes: [string, any][] = [
    ["Event", Event],
    ["CustomEvent", CustomEvent],
    ["EventTarget", EventTarget],
    ["MessageEvent", MessageEvent],
    ["MessagePort", MessagePort],
    ["MessageChannel", MessageChannel],
    ["BroadcastChannel", BroadcastChannel],
    ["ByteLengthQueuingStrategy", ByteLengthQueuingStrategy],
    ["CountQueuingStrategy", CountQueuingStrategy],
    ["ReadableStream", ReadableStream],
    ["ReadableStreamDefaultController", ReadableStreamDefaultController],
    ["ReadableStreamDefaultReader", ReadableStreamDefaultReader],
    ["WritableStream", WritableStream],
    ["WritableStreamDefaultController", WritableStreamDefaultController],
    ["WritableStreamDefaultWriter", WritableStreamDefaultWriter],
    ["TransformStream", TransformStream],
    ["TransformStreamDefaultController", TransformStreamDefaultController],
  ];
  for (const [name, value] of classes) {
    // Own and hollow -- a constructor whose prototype lacks the members node's has -- counts
    // as absent: the interpreter's stubs were exactly that.
    const existing = G[name];
    const hollow = typeof existing === "function" && Object.getOwnPropertyNames(existing.prototype || {}).length <= 1;
    if (typeof existing === "undefined" || hollow) {
      Object.defineProperty(G, name, { value, writable: true, enumerable: false, configurable: true });
    }
  }
})();
