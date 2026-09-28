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

// What a microtask checkpoint would run now, as bits: 1 a `process.nextTick` callback, 2 a
// promise job or `queueMicrotask` callback, 4 a rejection nothing handled, which the loop reports
// once the checkpoint is over (ADR 0065, representation: the realm's queues). And the realm's I/O
// lane: `fn(arg)` in node's poll phase, after the timers due when a turn begins and before its
// immediates, which is where node delivers a port's messages. Captured here, where message
// delivery is the one user of both.
const __kilnJobsPending: () => number = (globalThis as any).__kilnJobsPending;
const __kilnQueueIo: (fn: (arg: any) => void, arg: any) => void = (globalThis as any).__kilnQueueIo;
delete (globalThis as any).__kilnJobsPending;
delete (globalThis as any).__kilnQueueIo;
// The runtime's cross-realm ports and workers (`kiln_runtime::workers`, realms issue 06):
// `__kilnWorkerOp(op, ...)` is every synchronous operation on them, numbered below, and
// `__kilnPortWait(endpoint, referenced)` the one that waits -- a host operation, answered
// `{ requestId, promise }`, done once the endpoint has something queued or its peer has closed.
const __kilnWorkerOp: (op: number, ...args: any[]) => any = (globalThis as any).__kilnWorkerOp;
const __kilnEncodeMessageData: ((value: any) => any[] | undefined) | undefined =
  (globalThis as any).__kilnEncodeMessageData;
const __kilnPortWait: (endpoint: number, referenced: boolean) => { requestId: number; promise: Promise<any> } =
  (globalThis as any).__kilnPortWait;
delete (globalThis as any).__kilnWorkerOp;
delete (globalThis as any).__kilnEncodeMessageData;
delete (globalThis as any).__kilnPortWait;

(function installWebEventsAndStreams() {
  const G = globalThis as any;

  function invalidState(message: string): Error {
    const DOMExc = G.DOMException;
    if (typeof DOMExc === "function") return new DOMExc(message, "InvalidStateError");
    const error: any = new Error(message);
    error.name = "InvalidStateError";
    return error;
  }

  // The `_`-prefixed fields of the stream classes below are internal slots: node's objects have
  // no own enumerable properties, and `Object.keys(new ReadableStream())` is `[]` there. The
  // event and message classes use private fields instead, which are not properties at all: this
  // redefinition costs a `defineProperty` per field per object, and on the message path -- an
  // `Event` and a `MessageEvent` per message -- it was most of the time a message took.
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

  function typeError(message: string, code: string): Error {
    const error: any = new TypeError(message);
    error.code = code;
    return error;
  }

  // Captured once: a program that replaces `setImmediate` or `performance` (a fake-timer
  // library does) does not change when node delivers a message, closes a port or stamps an
  // event.
  const schedule: (fn: (arg: any) => void, arg: any) => void =
    typeof G.setImmediate === "function" ? G.setImmediate
      : typeof G.setTimeout === "function" ? (fn, arg) => G.setTimeout(fn, 0, arg)
        : (fn, arg) => G.queueMicrotask(() => fn(arg));
  const deliverLater: (fn: (arg: any) => void, arg: any) => void =
    typeof __kilnQueueIo === "function" ? __kilnQueueIo : schedule;
  const performanceNow: () => number = typeof G.performance?.now === "function"
    ? G.performance.now.bind(G.performance) : Date.now;
  const clone = __kilnStructuredClone;

  // ---- Event, CustomEvent, EventTarget ------------------------------------------------------

  interface Listener {
    callback: any;
    capture: boolean;
    once: boolean;
    passive: boolean;
    removed: boolean;
    // An `on<type>` handler attribute: a listener whose callback the attribute replaces in
    // place, so it keeps the position it was first given, as node's does.
    attribute: boolean;
  }

  // The dispatch algorithm and the event internals it writes, defined inside `Event` where the
  // private fields are reachable, and the listener table, defined inside `EventTarget`.
  let dispatchTo: (target: any, event: any, list: Listener[] | undefined) => boolean;
  let listenersOf: (target: any) => Map<string, Listener[]>;

  // Node keeps four of an event's slots as own enumerable properties under symbols of its own,
  // so `Reflect.ownKeys(new Event("x"))` lists them and `{ ...event }` copies them, and a
  // CustomEvent's detail as a fifth. These are those symbols, with node's descriptions, and the
  // properties are the state itself as they are there: writing one is what `event.type` reads,
  // and an object is an event when it has a string under `kType` (node's `isEvent`). The rest
  // is private: the booleans are bits of one number, and the phase and current target follow
  // from whether a dispatch is running (an event here is only ever at its target). A symbol-keyed
  // define or store takes the native object model's generic path, and the four of them are most
  // of what an event costs there over the private fields they replaced (issue 14).
  const kType = Symbol("type");
  const kTarget = Symbol("kTarget");
  const kIsBeingDispatched = Symbol("kIsBeingDispatched");
  const kInPassiveListener = Symbol("kInPassiveListener");
  const kDetail = Symbol("detail");
  const BUBBLES = 1, CANCELABLE = 2, COMPOSED = 4, DEFAULT_PREVENTED = 8, STOP = 16, STOP_IMMEDIATE = 32;

  // Node's `util.inspect` spelling of a string (`strEscape`): single quotes unless the text has
  // one, control characters escaped.
  function inspectString(text: string): string {
    let quote = "'";
    if (text.includes("'")) {
      if (!text.includes('"')) quote = '"';
      else if (!text.includes("`") && !text.includes("${")) quote = "`";
    }
    let out = quote;
    for (let i = 0; i < text.length; i++) {
      const ch = text[i];
      const code = text.charCodeAt(i);
      if (ch === quote || ch === "\\") out += "\\" + ch;
      else if (code === 8) out += "\\b";
      else if (code === 9) out += "\\t";
      else if (code === 10) out += "\\n";
      else if (code === 12) out += "\\f";
      else if (code === 13) out += "\\r";
      else if (code < 0x20 || code === 0x7f) out += "\\x" + (code < 16 ? "0" : "") + code.toString(16).toUpperCase();
      else out += ch;
    }
    return out + quote;
  }

  // `util.inspect` of a flat object of primitives as node lays it out: on one line when it fits
  // in `breakLength` (80 by default), with node's allowance for the braces and separators, and
  // one entry a line otherwise.
  function inspectFlat(entries: string[], options: any): string {
    const breakLength = options && typeof options.breakLength === "number" ? options.breakLength : 80;
    let total = entries.length + entries.length + 15;
    for (const entry of entries) total += entry.length;
    return total <= breakLength
      ? "{ " + entries.join(", ") + " }"
      : "{\n  " + entries.join(",\n  ") + "\n}";
  }

  function eventThis(value: any): void {
    if (value === null || value === undefined || typeof value[kType] !== "string") {
      throw typeError("Value of \"this\" must be of type Event", "ERR_INVALID_THIS");
    }
  }

  class Event {
    [kType] = "";
    [kTarget]: any = null;
    [kIsBeingDispatched] = false;
    [kInPassiveListener] = false;
    #flags: number;
    #timeStamp: number;

    static NONE = 0;
    static CAPTURING_PHASE = 1;
    static AT_TARGET = 2;
    static BUBBLING_PHASE = 3;

    constructor(type: any, init?: any) {
      if (type === undefined && arguments.length === 0) throw new TypeError("The \"type\" argument must be specified");
      this[kType] = typeof type === "string" ? type : String(type);
      this.#flags = init === undefined || init === null ? 0
        : (init.bubbles ? BUBBLES : 0) | (init.cancelable ? CANCELABLE : 0) | (init.composed ? COMPOSED : 0);
      this.#timeStamp = performanceNow();
    }

    get type(): string { eventThis(this); return this[kType]; }
    get bubbles(): boolean { return (this.#flags & BUBBLES) !== 0; }
    get cancelable(): boolean { return (this.#flags & CANCELABLE) !== 0; }
    get composed(): boolean { return (this.#flags & COMPOSED) !== 0; }
    get defaultPrevented(): boolean { return (this.#flags & DEFAULT_PREVENTED) !== 0; }
    get target(): any { eventThis(this); return this[kTarget]; }
    get srcElement(): any { eventThis(this); return this[kTarget]; }
    get currentTarget(): any { eventThis(this); return this[kIsBeingDispatched] ? this[kTarget] : null; }
    get eventPhase(): number { eventThis(this); return this[kIsBeingDispatched] ? 2 : 0; }
    get timeStamp(): number { return this.#timeStamp; }
    get isTrusted(): boolean { return false; }
    get returnValue(): boolean { return !this.defaultPrevented; }
    set returnValue(value: boolean) { if (!value) this.preventDefault(); }
    get cancelBubble(): boolean { return (this.#flags & STOP) !== 0; }
    set cancelBubble(value: boolean) { if (value) this.#flags |= STOP; }

    preventDefault(): void {
      if ((this.#flags & CANCELABLE) !== 0 && !this[kInPassiveListener]) this.#flags |= DEFAULT_PREVENTED;
    }
    stopPropagation(): void { this.#flags |= STOP; }
    stopImmediatePropagation(): void { this.#flags |= STOP | STOP_IMMEDIATE; }
    composedPath(): any[] { eventThis(this); return this[kIsBeingDispatched] && this[kTarget] ? [this[kTarget]] : []; }
    initEvent(type: any, bubbles?: any, cancelable?: any): void {
      eventThis(this);
      if (this[kIsBeingDispatched]) return;
      this[kType] = String(type);
      this.#flags = (this.#flags & ~(BUBBLES | CANCELABLE)) | (bubbles ? BUBBLES : 0) | (cancelable ? CANCELABLE : 0);
    }
    get [Symbol.toStringTag](): string { return "Event"; }

    // Node's `util.inspect.custom`: the class and the four fields node shows.
    [Symbol.for("nodejs.util.inspect.custom")](depth: number, options: any): string {
      const name = this.constructor.name;
      if (depth < 0) return name;
      const timeStamp = this.#timeStamp;
      return name + " " + inspectFlat([
        "type: " + inspectString(this[kType]),
        "defaultPrevented: " + this.defaultPrevented,
        "cancelable: " + this.cancelable,
        "timeStamp: " + (Object.is(timeStamp, -0) ? "-0" : String(timeStamp)),
      ], options);
    }

    static {
      dispatchTo = (target: any, event: Event, list: Listener[] | undefined): boolean => {
        if (event[kIsBeingDispatched]) throw invalidState("The event is already being dispatched");
        event[kIsBeingDispatched] = true;
        event[kTarget] = target;
        if (list !== undefined && list.length !== 0) {
          // A snapshot: a listener added during dispatch does not run, one removed does not.
          const snapshot = list.length === 1 ? null : list.slice();
          const count = snapshot === null ? 1 : snapshot.length;
          for (let i = 0; i < count; i++) {
            const listener = snapshot === null ? list[0] : snapshot[i];
            if (listener.removed) continue;
            if (listener.once) removeListener(list, listener);
            const callback = listener.callback;
            if (listener.passive) event[kInPassiveListener] = true;
            try {
              if (typeof callback === "function") callback.call(target, event);
              else if (callback && typeof callback.handleEvent === "function") callback.handleEvent(event);
            } catch (error) {
              // A throwing listener does not stop the others; node reports it as uncaught.
              reportUncaught(error);
            }
            if (listener.passive) event[kInPassiveListener] = false;
            if ((event.#flags & STOP_IMMEDIATE) !== 0) break;
          }
        }
        event[kIsBeingDispatched] = false;
        return (event.#flags & DEFAULT_PREVENTED) === 0;
      };
    }
  }
  for (const [name, value] of [["NONE", 0], ["CAPTURING_PHASE", 1], ["AT_TARGET", 2], ["BUBBLING_PHASE", 3]] as [string, number][]) {
    Object.defineProperty(Event.prototype, name, { value, writable: false, enumerable: true, configurable: false });
  }

  class CustomEvent extends Event {
    [kDetail]: any;
    constructor(type: any, init?: any) {
      super(type, init);
      this[kDetail] = init && init.detail !== undefined ? init.detail : null;
    }
    get detail(): any { eventThis(this); return this[kDetail]; }
    get [Symbol.toStringTag](): string { return "CustomEvent"; }
  }

  function removeListener(list: Listener[], listener: Listener): void {
    listener.removed = true;
    const at = list.indexOf(listener);
    if (at !== -1) list.splice(at, 1);
  }

  function optionsOf(options: any): { capture: boolean; once: boolean; passive: boolean; signal: any } {
    if (typeof options === "boolean") return { capture: options, once: false, passive: false, signal: undefined };
    if (options === null || typeof options !== "object") return { capture: false, once: false, passive: false, signal: undefined };
    return { capture: !!options.capture, once: !!options.once, passive: !!options.passive, signal: options.signal };
  }

  function captureOf(options: any): boolean {
    if (typeof options === "boolean") return options;
    return options !== null && typeof options === "object" && !!options.capture;
  }

  class EventTarget {
    #listeners: Map<string, Listener[]> = new Map();

    addEventListener(type: any, callback: any, options?: any): void {
      const table = this.#listeners;
      if (callback === null || callback === undefined) return;
      const parsed = optionsOf(options);
      if (parsed.signal !== undefined && parsed.signal !== null && parsed.signal.aborted) return;
      const key = String(type);
      let list = table.get(key);
      if (!list) {
        list = [];
        table.set(key, list);
      }
      for (const existing of list) {
        if (!existing.attribute && existing.callback === callback && existing.capture === parsed.capture) return;
      }
      const listener: Listener = { callback, capture: parsed.capture, once: parsed.once, passive: parsed.passive, removed: false, attribute: false };
      list.push(listener);
      if (parsed.signal !== undefined && parsed.signal !== null && typeof parsed.signal.addEventListener === "function") {
        parsed.signal.addEventListener("abort", () => this.removeEventListener(key, callback, { capture: parsed.capture }), { once: true });
      }
    }

    removeEventListener(type: any, callback: any, options?: any): void {
      const list = this.#listeners.get(String(type));
      if (!list) return;
      const capture = captureOf(options);
      for (let i = 0; i < list.length; i++) {
        if (!list[i].attribute && list[i].callback === callback && list[i].capture === capture) {
          removeListener(list, list[i]);
          return;
        }
      }
    }

    dispatchEvent(event: any): boolean {
      if (!(event instanceof Event)) {
        throw new TypeError("The \"event\" argument must be an instance of Event. Received " + typeof event);
      }
      return dispatchTo(this, event, this.#listeners.get(event.type));
    }
    get [Symbol.toStringTag](): string { return "EventTarget"; }

    static {
      listenersOf = (target: any) => target.#listeners;
    }
  }

  // The `on<type>` attribute: a function installs or replaces the attribute's listener, anything
  // else removes it.
  function setAttributeHandler(target: any, type: string, handler: any, current: Listener | null): Listener | null {
    const list = listenersOf(target);
    if (typeof handler !== "function") {
      if (current !== null) {
        const entries = list.get(type);
        if (entries) removeListener(entries, current);
      }
      return null;
    }
    if (current !== null && !current.removed) {
      current.callback = handler;
      return current;
    }
    let entries = list.get(type);
    if (!entries) {
      entries = [];
      list.set(type, entries);
    }
    const listener: Listener = { callback: handler, capture: false, once: false, passive: false, removed: false, attribute: true };
    entries.push(listener);
    return listener;
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

  // ---- MessageEvent, MessageChannel, MessagePort, BroadcastChannel ---------------------------

  let messageEvent: (data: any, ports: any[] | null) => MessageEvent;

  // `origin`, `lastEventId` and `source` are never set on a port's message, so they share one
  // field that stays null unless a program constructs a MessageEvent with them.
  class MessageEvent extends Event {
    #data: any;
    // Null until read: a port's message rarely carries ports, and the empty list is made on
    // first read and kept, so `event.ports === event.ports`.
    #ports: any[] | null;
    #meta: { origin: string; lastEventId: string; source: any } | null;
    constructor(type: any, init?: any) {
      super(type, init);
      if (init === undefined || init === null) {
        this.#data = null;
        this.#ports = null;
        this.#meta = null;
      } else {
        this.#data = init.data === undefined ? null : init.data;
        this.#ports = init.ports ? Array.from(init.ports) : [];
        this.#meta = init.origin === undefined && init.lastEventId === undefined && init.source === undefined ? null : {
          origin: init.origin === undefined ? "" : String(init.origin),
          lastEventId: init.lastEventId === undefined ? "" : String(init.lastEventId),
          source: init.source === undefined ? null : init.source,
        };
      }
    }
    get data(): any { return this.#data; }
    get origin(): string { return this.#meta === null ? "" : this.#meta.origin; }
    get lastEventId(): string { return this.#meta === null ? "" : this.#meta.lastEventId; }
    get source(): any { return this.#meta === null ? null : this.#meta.source; }
    get ports(): any[] {
      if (this.#ports === null) this.#ports = [];
      return this.#ports;
    }
    get [Symbol.toStringTag](): string { return "MessageEvent"; }

    static {
      messageEvent = (data: any, ports: any[] | null) => {
        const event = new MessageEvent("message", undefined);
        event.#data = data;
        event.#ports = ports;
        return event;
      };
    }
  }

  const NO_PORTS: any[] = [];
  // A queued `close()` of the other end: delivered in order after the messages already queued,
  // which is why messages posted just before `port1.close()` still reach `port2` in node.
  const CLOSE_MARK = {};

  // ---- Messages between realms (ADR 0070 primitive A; realms issue 06) ------------------------
  //
  // A port whose other end lives in another realm -- a worker's `parentPort`, a port transferred
  // to or from a worker, a `BroadcastChannel` -- is bound to an endpoint of the runtime's
  // (`kiln_runtime::workers`). A message crosses as the structured clone above, flattened into a
  // list of tokens the runtime can carry: numbers, strings, `undefined`/`null`/booleans, and
  // buffers. The flattening is a wire format over a value the clone has already made, not a
  // second clone: every object in it is a fresh copy the sender holds no reference to, which is
  // exactly the proof ADR 0070 asks before a graph's storage moves -- so an `ArrayBuffer` in it
  // hands its backing store to the runtime (and on to the receiving realm) instead of copying it.
  //
  // Each token list is a tag followed by what the tag needs. An object is numbered when it is
  // first written, and written again as a reference to that number, which keeps shared
  // references and cycles.
  const T_UNDEFINED = 0, T_NULL = 1, T_FALSE = 2, T_TRUE = 3, T_NUMBER = 4, T_STRING = 5, T_BIGINT = 6;
  const T_REF = 7, T_OBJECT = 8, T_ARRAY = 9, T_HOLE = 10, T_DATE = 11, T_REGEXP = 12, T_MAP = 13;
  const T_SET = 14, T_BUFFER = 15, T_VIEW = 16, T_DATA_VIEW = 17, T_SHARED = 18, T_ERROR = 19;
  const T_BOXED = 20, T_PORT = 21;

  // The runtime's operations (`__kilnWorkerOp`); the numbers are the engines' too.
  const OP_CHANNEL = 1, OP_POST = 2, OP_TAKE = 3, OP_CLOSE = 4, OP_STATE = 5, OP_ENQUEUE = 6;
  const OP_BROADCAST_JOIN = 7, OP_BROADCAST_POST = 8, OP_SPAWN = 9, OP_TERMINATE = 10, OP_SELF = 11;
  const OP_CANCEL_WAIT = 12, OP_REAP = 13, OP_STDIO = 14;
  // `kiln_runtime::workers::kind` and `STATE_*`.
  const KIND_USER = 0, KIND_TERMINATE = 7;
  const STATE_PEER_CLOSED = 2, STATE_GONE = 4;

  const objectTag = Object.prototype.toString;
  const isArrayValue = Array.isArray;
  const objectKeys = Object.keys;

  // A port that was transferred: its endpoint, standing where the port was in the copy.
  class PortHandle {
    endpoint: number;
    constructor(endpoint: number) {
      this.endpoint = endpoint;
    }
  }

  function encodeInto(value: any, out: any[], seen: Map<any, number>): void {
    switch (typeof value) {
      case "undefined": out.push(T_UNDEFINED); return;
      case "boolean": out.push(value ? T_TRUE : T_FALSE); return;
      case "number": out.push(T_NUMBER, value); return;
      case "string": out.push(T_STRING, value); return;
      case "bigint": out.push(T_BIGINT, value.toString()); return;
    }
    if (value === null) {
      out.push(T_NULL);
      return;
    }
    const known = seen.get(value);
    if (known !== undefined) {
      out.push(T_REF, known);
      return;
    }
    seen.set(value, seen.size);
    if (isArrayValue(value)) {
      const length = value.length;
      out.push(T_ARRAY, length);
      for (let i = 0; i < length; i++) {
        if (i in value) encodeInto(value[i], out, seen);
        else out.push(T_HOLE);
      }
      return;
    }
    if (value instanceof PortHandle) {
      out.push(T_PORT, value.endpoint);
      return;
    }
    const tag = objectTag.call(value);
    switch (tag) {
      case "[object Object]": break;
      case "[object ArrayBuffer]": out.push(T_BUFFER, value); return;
      case "[object SharedArrayBuffer]": out.push(T_SHARED, value); return;
      case "[object DataView]":
        out.push(T_DATA_VIEW, value.byteOffset, value.byteLength);
        encodeInto(value.buffer, out, seen);
        return;
      case "[object Date]": out.push(T_DATE, value.getTime()); return;
      case "[object RegExp]": out.push(T_REGEXP, value.source, value.flags); return;
      case "[object Map]": {
        out.push(T_MAP, value.size);
        for (const [k, v] of value) {
          encodeInto(k, out, seen);
          encodeInto(v, out, seen);
        }
        return;
      }
      case "[object Set]": {
        out.push(T_SET, value.size);
        for (const v of value) encodeInto(v, out, seen);
        return;
      }
      case "[object Error]": {
        out.push(T_ERROR, String(value.name), String(value.message));
        encodeInto("stack" in value ? value.stack : undefined, out, seen);
        const hasCause = "cause" in value;
        out.push(hasCause ? 1 : 0);
        if (hasCause) encodeInto(value.cause, out, seen);
        return;
      }
      case "[object Boolean]": out.push(T_BOXED, T_TRUE); encodeInto(value.valueOf(), out, seen); return;
      case "[object Number]": out.push(T_BOXED, T_NUMBER); encodeInto(value.valueOf(), out, seen); return;
      case "[object String]": out.push(T_BOXED, T_STRING); encodeInto(value.valueOf(), out, seen); return;
      case "[object BigInt]": out.push(T_BOXED, T_BIGINT); encodeInto(value.valueOf(), out, seen); return;
      default:
        if (ArrayBuffer.isView(value)) {
          out.push(T_VIEW, tag.slice(8, -1), value.byteOffset, value.length);
          encodeInto(value.buffer, out, seen);
          return;
        }
    }
    // What the clone made of any other object: an ordinary one, its own enumerable properties.
    const keys = objectKeys(value);
    out.push(T_OBJECT, keys.length);
    for (let i = 0; i < keys.length; i++) {
      out.push(keys[i]);
      encodeInto(value[keys[i]], out, seen);
    }
  }

  const errorConstructors: Record<string, any> = {
    Error, EvalError, RangeError, ReferenceError, SyntaxError, TypeError, URIError,
  };

  interface DecodeState {
    tokens: any[];
    at: number;
    objects: any[];
    ports: Map<number, MessagePort>;
  }

  function decodeValue(state: DecodeState): any {
    const tokens = state.tokens;
    const tag = tokens[state.at++];
    switch (tag) {
      case T_UNDEFINED: return undefined;
      case T_NULL: return null;
      case T_FALSE: return false;
      case T_TRUE: return true;
      case T_NUMBER:
      case T_STRING: return tokens[state.at++];
      case T_BIGINT: return BigInt(tokens[state.at++]);
      case T_REF: return state.objects[tokens[state.at++]];
      case T_ARRAY: {
        const length = tokens[state.at++];
        const out = new Array(length);
        state.objects.push(out);
        for (let i = 0; i < length; i++) {
          if (tokens[state.at] === T_HOLE) {
            state.at++;
            continue;
          }
          out[i] = decodeValue(state);
        }
        return out;
      }
      case T_OBJECT: {
        const count = tokens[state.at++];
        const out: any = {};
        state.objects.push(out);
        for (let i = 0; i < count; i++) {
          const key = tokens[state.at++];
          const item = decodeValue(state);
          if (key === "__proto__") {
            Object.defineProperty(out, key, { value: item, writable: true, enumerable: true, configurable: true });
          } else {
            out[key] = item;
          }
        }
        return out;
      }
      case T_BUFFER:
      case T_SHARED: {
        const buffer = tokens[state.at++];
        state.objects.push(buffer);
        return buffer;
      }
      case T_VIEW: {
        const name = tokens[state.at++];
        const offset = tokens[state.at++];
        const length = tokens[state.at++];
        const slot = state.objects.length;
        state.objects.push(undefined);
        const buffer = decodeValue(state);
        const Ctor = G[name];
        const view = new Ctor(buffer, offset, length);
        state.objects[slot] = view;
        return view;
      }
      case T_DATA_VIEW: {
        const offset = tokens[state.at++];
        const length = tokens[state.at++];
        const slot = state.objects.length;
        state.objects.push(undefined);
        const view = new DataView(decodeValue(state), offset, length);
        state.objects[slot] = view;
        return view;
      }
      case T_DATE: {
        const out = new Date(tokens[state.at++]);
        state.objects.push(out);
        return out;
      }
      case T_REGEXP: {
        const source = tokens[state.at++];
        const out = new RegExp(source, tokens[state.at++]);
        state.objects.push(out);
        return out;
      }
      case T_MAP: {
        const count = tokens[state.at++];
        const out = new Map();
        state.objects.push(out);
        for (let i = 0; i < count; i++) {
          const key = decodeValue(state);
          out.set(key, decodeValue(state));
        }
        return out;
      }
      case T_SET: {
        const count = tokens[state.at++];
        const out = new Set();
        state.objects.push(out);
        for (let i = 0; i < count; i++) out.add(decodeValue(state));
        return out;
      }
      case T_ERROR: {
        const name = tokens[state.at++];
        const message = tokens[state.at++];
        const Ctor = errorConstructors[name] || Error;
        const out: any = new Ctor(message);
        state.objects.push(out);
        if (Ctor === Error && name !== "Error") {
          Object.defineProperty(out, "name", { value: name, writable: true, enumerable: false, configurable: true });
        }
        const stack = decodeValue(state);
        if (stack !== undefined) {
          Object.defineProperty(out, "stack", { value: stack, writable: true, enumerable: false, configurable: true });
        }
        if (tokens[state.at++] === 1) {
          Object.defineProperty(out, "cause", { value: decodeValue(state), writable: true, enumerable: false, configurable: true });
        }
        return out;
      }
      case T_BOXED: {
        tokens[state.at++];
        const slot = state.objects.length;
        state.objects.push(undefined);
        const out = Object(decodeValue(state));
        state.objects[slot] = out;
        return out;
      }
      case T_PORT: {
        const endpoint = tokens[state.at++];
        let port = state.ports.get(endpoint);
        if (port === undefined) {
          port = remotePort(endpoint);
          state.ports.set(endpoint, port);
        }
        state.objects.push(port);
        return port;
      }
    }
    throw clone.dataCloneError("Unable to deserialize cloned data.");
  }

  // A message on the wire: the transferred ports' endpoints (the event's `ports`), then the
  // value. `source` is the port posting it, which may not transfer itself.
  function encodeMessage(message: any, list: any[] | undefined, source: any): any[] {
    let copy: any;
    const handles: PortHandle[] = [];
    if (list === undefined || list.length === 0) {
      if (typeof message !== "object" || message === null) {
        if (typeof message === "function" || typeof message === "symbol") return encodeUncloneable(message);
        const out: any[] = [0];
        encodeInto(message, out, new Map());
        return out;
      }
      if (typeof __kilnEncodeMessageData === "function") {
        const encoded = __kilnEncodeMessageData(message);
        if (encoded !== undefined) return encoded;
      }
      copy = clone.serialize(message, undefined);
    } else {
      copy = clone.serialize(message, list, (item: any) => {
        if (!(item instanceof MessagePort)) return undefined;
        if (item === source) throw clone.dataCloneError("Transfer list contains source port");
        return () => {
          const handle = new PortHandle(detachPort(item));
          handles.push(handle);
          return handle;
        };
      });
    }
    const out: any[] = [handles.length];
    for (let i = 0; i < handles.length; i++) out.push(handles[i].endpoint);
    encodeInto(copy, out, new Map());
    return out;
  }

  // A function or symbol at the top of a message throws the clone's own error.
  function encodeUncloneable(message: any): never {
    clone.serialize(message, undefined);
    throw clone.dataCloneError("could not be cloned.");
  }

  // `[value, ports]` back from a message's tokens.
  function decodeMessage(tokens: any[]): [any, any[]] {
    const count = tokens[0];
    const state: DecodeState = { tokens, at: 1 + count, objects: [], ports: new Map() };
    const ports: any[] = count === 0 ? NO_PORTS : [];
    for (let i = 0; i < count; i++) {
      const endpoint = tokens[1 + i];
      const port = remotePort(endpoint);
      state.ports.set(endpoint, port);
      ports.push(port);
    }
    return [decodeValue(state), ports];
  }

  function decodeRemote(got: any[]): [any, any[]] {
    // The native adapter can already have built a plain graph or buffer in the receiving heap.
    // Other kinds and transferred ports still use the shared TypeScript decoder.
    return got[2] === true ? [got[1], NO_PORTS] : decodeMessage(got[1]);
  }

  let deliverNext: (port: MessagePort) => void;
  let continueFromTick: (port: MessagePort) => void;
  let continueFromMicrotask: (port: MessagePort) => void;
  let transferPort: (port: MessagePort) => MessagePort;
  let detachPort: (port: MessagePort) => number;
  let remotePort: (endpoint: number) => MessagePort;
  let listen: (port: MessagePort) => void;
  let stopWaiting: (port: MessagePort) => void;
  let entangle: (a: MessagePort, b: MessagePort) => void;
  // A port the runtime-side protocol reads: a worker's own channel, its control endpoint, a
  // broadcast group. `kinds` takes every message, the protocol's and the program's.
  let internalPort: (endpoint: number, kinds: (kind: number, value: any, ports: any[]) => void,
    target: any, broadcast: boolean) => MessagePort;
  let portEndpoint: (port: MessagePort) => number;
  // Deliver what the runtime holds for `port` from this turn's I/O callbacks, without waiting
  // for its wait to come back: a post that the sender knows reached a port of its own realm.
  let kick: (port: MessagePort) => void;

  // Node delivers a port's messages from one libuv callback, dispatching each as its own
  // callback with a microtask checkpoint -- `process.nextTick` callbacks, then promise jobs,
  // until both are empty -- before the next. So a delivery here is one macrotask that dispatches
  // messages in posting order and, after each, asks what a checkpoint would run. Nothing: the
  // checkpoint is empty and the next message goes out at once, exactly as in node. Something:
  // the delivery continues from the back of the checkpoint -- behind the ticks while there are
  // ticks, behind the promise jobs while there are those, and always from a tick at the last
  // step, so the next message's own ticks still run before its promise jobs. No other macrotask
  // gets between two messages of one batch, which is node's order too.
  //
  // The queue holds `data, ports` pairs; `NO_PORTS` stands for an empty list until an event
  // needs one of its own. A port bound to another realm takes its messages from the runtime as
  // it dispatches them, after anything still in its own queue.
  const DELIVERY_BATCH = 1000;
  const CONTINUATION_HOPS = 64;
  const TICKS = 1;
  const MICROTASKS = 2;
  const REJECTION = 4;
  const nextTick: ((fn: (arg: any) => void, arg: any) => void) | null =
    G.process && typeof G.process.nextTick === "function" ? G.process.nextTick : null;
  const queueMicrotask: (fn: () => void) => void = G.queueMicrotask;
  class MessagePort extends EventTarget {
    #other: MessagePort | null = null;
    #queue: any[] = [];
    #head = 0;
    #started = false;
    #closed = false;
    #scheduled = false;
    #budget = 0;
    #hops = 0;
    // The runtime endpoint when the other end is in another realm, 0 otherwise; the pending
    // wait on it; whether that wait keeps the realm alive.
    #remote = 0;
    #wait: any = null;
    #unref = false;
    #broadcast = false;
    // The protocol reader of an internal port, and where its events go (a BroadcastChannel).
    #kinds: ((kind: number, value: any, ports: any[]) => void) | null = null;
    #target: any = null;
    // The `message` listener list, once there is one: the table keeps one array per type and
    // edits it in place, so the lookup -- a string-keyed Map read per message -- is made once.
    #messageListeners: Listener[] | undefined = undefined;
    #onmessage: Listener | null = null;
    #onmessageerror: Listener | null = null;

    get onmessage(): any { return this.#onmessage === null ? null : this.#onmessage.callback; }
    set onmessage(handler: any) {
      this.#onmessage = setAttributeHandler(this, "message", handler, this.#onmessage);
      this.#autoRef();
      if (this.#onmessage !== null) this.start();
    }
    get onmessageerror(): any { return this.#onmessageerror === null ? null : this.#onmessageerror.callback; }
    set onmessageerror(handler: any) {
      this.#onmessageerror = setAttributeHandler(this, "messageerror", handler, this.#onmessageerror);
    }

    // Node starts a port for a `message` listener as it does for `onmessage`, and a port with
    // one keeps its realm alive until the last is removed: a worker whose `parentPort` listener
    // was a `once` ends after that message.
    addEventListener(type: any, callback: any, options?: any): void {
      super.addEventListener(type, callback, options);
      if (callback !== null && callback !== undefined && String(type) === "message") {
        this.#autoRef();
        this.start();
      }
    }
    removeEventListener(type: any, callback: any, options?: any): void {
      super.removeEventListener(type, callback, options);
      if (String(type) === "message") this.#autoRef();
    }
    #autoRef(): void {
      if (this.#kinds !== null || this.#target !== null) return;
      const list = listenersOf(this).get("message");
      if (list !== undefined && list.length !== 0) this.ref();
      else this.unref();
    }

    // Answers `true`, as node's does, unless this port is closed; a copy is made -- and a
    // transfer list detached -- either way.
    postMessage(message: any, transfer?: any): boolean | undefined {
      if (message === undefined && arguments.length === 0) {
        throw typeError("Not enough arguments to MessagePort.postMessage", "ERR_MISSING_ARGS");
      }
      let list: any[] | undefined;
      if (transfer !== undefined && transfer !== null) {
        if (typeof transfer !== "object") {
          throw typeError("Optional transferList argument must be an iterable", "ERR_INVALID_ARG_TYPE");
        }
        list = typeof transfer[Symbol.iterator] === "function"
          ? clone.sequence(transfer, "Optional transferList argument must be an iterable")
          : clone.transferOption(transfer, "postMessage");
      }
      if (this.#remote !== 0) {
        const tokens = encodeMessage(message, list, this);
        if (this.#closed) return undefined;
        if (this.#broadcast) __kilnWorkerOp(OP_BROADCAST_POST, this.#remote, tokens);
        else __kilnWorkerOp(OP_POST, this.#remote, KIND_USER, tokens);
        return true;
      }
      let data: any;
      let ports = NO_PORTS;
      if (list === undefined || list.length === 0) {
        data = clone.serialize(message, undefined);
      } else {
        const moved: MessagePort[] = [];
        data = clone.serialize(message, list, (item: any) => {
          if (!(item instanceof MessagePort)) return undefined;
          if (item === this) throw clone.dataCloneError("Transfer list contains source port");
          return () => {
            const port = transferPort(item);
            moved.push(port);
            return port;
          };
        });
        if (moved.length !== 0) ports = moved;
      }
      if (this.#closed) return undefined;
      const other = this.#other;
      if (other === null) return true;
      other.#queue.push(data, ports);
      if (other.#started && !other.#scheduled) {
        other.#scheduled = true;
        deliverLater(deliverNext, other);
      }
      return true;
    }

    start(): void {
      if (this.#started) return;
      this.#started = true;
      if (this.#head < this.#queue.length && !this.#scheduled) {
        this.#scheduled = true;
        deliverLater(deliverNext, this);
      } else if (this.#remote !== 0) {
        listen(this);
      }
    }

    close(): void {
      if (this.#closed) return;
      this.#closed = true;
      this.#queue = [];
      this.#head = 0;
      if (this.#remote !== 0) {
        const remote = this.#remote;
        this.#remote = 0;
        stopWaiting(this);
        __kilnWorkerOp(OP_CLOSE, remote);
      }
      const other = this.#other;
      this.#other = null;
      if (other !== null && !other.#closed) {
        other.#other = null;
        other.#queue.push(CLOSE_MARK, NO_PORTS);
        if (other.#started && !other.#scheduled) {
          other.#scheduled = true;
          deliverLater(deliverNext, other);
        }
      }
      schedule(closed, this);
    }
    // A port bound to another realm keeps this one alive while it is listening, as node's does;
    // one in this realm has nothing another realm could send it.
    ref(): this {
      if (this.#unref) {
        this.#unref = false;
        if (this.#wait !== null) {
          stopWaiting(this);
          listen(this);
        }
      }
      return this;
    }
    unref(): this {
      if (!this.#unref) {
        this.#unref = true;
        if (this.#wait !== null) {
          stopWaiting(this);
          listen(this);
        }
      }
      return this;
    }
    hasRef(): boolean { return this.#started && !this.#closed && !this.#unref; }
    get [Symbol.toStringTag](): string { return "MessagePort"; }

    // Node's `NodeEventTarget` surface, which its `MessagePort` has and a worker's `parentPort`
    // is mostly used through: a listener added with `on` is called with a message's data rather
    // than the event, and with nothing for any other event.
    #nodeStyle: { type: string; listener: any; wrapper: any }[] = [];
    #addNodeStyle(type: any, listener: any, once: boolean): this {
      if (typeof listener !== "function") {
        throw typeError(`The "listener" argument must be of type function. Received ${listener === null ? "null" : typeof listener}`, "ERR_INVALID_ARG_TYPE");
      }
      const key = String(type);
      const entries = this.#nodeStyle;
      const wrapper = function (this: any, event: any) {
        if (once) {
          const at = entries.findIndex((entry) => entry.wrapper === wrapper);
          if (at >= 0) entries.splice(at, 1);
        }
        return key === "message" || key === "messageerror" ? listener.call(this, event.data) : listener.call(this);
      };
      entries.push({ type: key, listener, wrapper });
      this.addEventListener(key, wrapper, once ? { once: true } : undefined);
      return this;
    }
    on(type: any, listener: any): this { return this.#addNodeStyle(type, listener, false); }
    addListener(type: any, listener: any): this { return this.#addNodeStyle(type, listener, false); }
    once(type: any, listener: any): this { return this.#addNodeStyle(type, listener, true); }
    off(type: any, listener: any): this { return this.removeListener(type, listener); }
    removeListener(type: any, listener: any): this {
      const key = String(type);
      const entries = this.#nodeStyle;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].type === key && entries[i].listener === listener) {
          this.removeEventListener(key, entries[i].wrapper);
          entries.splice(i, 1);
          break;
        }
      }
      return this;
    }
    removeAllListeners(type?: any): this {
      const entries = this.#nodeStyle;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (type === undefined || entries[i].type === String(type)) {
          this.removeEventListener(entries[i].type, entries[i].wrapper);
          entries.splice(i, 1);
        }
      }
      return this;
    }
    emit(type: any, arg?: any): boolean {
      const key = String(type);
      const list = listenersOf(this).get(key);
      const had = list !== undefined && list.some((entry) => !entry.removed);
      dispatchTo(this, key === "message" || key === "messageerror" ? new MessageEvent(key, { data: arg }) : new Event(key), list);
      return had;
    }
    listenerCount(type: any): number {
      const list = listenersOf(this).get(String(type));
      return list === undefined ? 0 : list.filter((entry) => !entry.removed).length;
    }
    eventNames(): string[] {
      const names: string[] = [];
      for (const [name, list] of listenersOf(this)) if (list.some((entry) => !entry.removed)) names.push(name);
      return names;
    }
    setMaxListeners(_n: number): this { return this; }
    getMaxListeners(): number { return 10; }

    static {
      // `receiveMessageOnPort` (node:worker_threads): take the oldest message not yet
      // dispatched, as a delivery would, without dispatching it. A queued close is not a message.
      const receive = (port: MessagePort): { message: any } | undefined => {
        if (port.#closed) return undefined;
        const queue = port.#queue;
        const at = port.#head;
        if (at >= queue.length) {
          if (port.#remote === 0) return undefined;
          const got = __kilnWorkerOp(OP_TAKE, port.#remote);
          if (got === undefined) return undefined;
          return { message: decodeRemote(got)[0] };
        }
        const data = queue[at];
        if (data === CLOSE_MARK) return undefined;
        queue[at] = undefined;
        queue[at + 1] = undefined;
        if (at + 2 === queue.length) {
          queue.length = 0;
          port.#head = 0;
        } else {
          port.#head = at + 2;
        }
        return { message: data };
      };
      Object.defineProperty(MessagePort, Symbol.for("kiln.messagePort"), {
        value: {
          receive,
          workerOp: __kilnWorkerOp,
          encodeMessage,
          decodeMessage,
          internalPort: (endpoint: number, kinds: any, target: any, broadcast: boolean) =>
            internalPort(endpoint, kinds, target, broadcast),
          portEndpoint: (port: MessagePort) => portEndpoint(port),
          worker: () => workerState,
        },
        writable: false,
        enumerable: false,
        configurable: false,
      });
      deliverNext = (port: MessagePort) => {
        port.#scheduled = false;
        port.#hops = 0;
        // Node's own bound: at least the messages that were waiting, and never fewer than a
        // thousand, so a port that keeps being written to cannot hold the loop forever.
        let budget = (port.#queue.length - port.#head) / 2;
        if (budget < DELIVERY_BATCH) budget = DELIVERY_BATCH;
        deliverSome(port, budget);
      };
      // The next message for `port` from the runtime, dispatched; false when there is none.
      const deliverRemote = (port: MessagePort): boolean => {
        const remote = port.#remote;
        const got = __kilnWorkerOp(OP_TAKE, remote);
        if (got === undefined) {
          if ((__kilnWorkerOp(OP_STATE, remote) & (STATE_PEER_CLOSED | STATE_GONE)) !== 0) {
            // The other end has gone and everything it sent has been delivered: this end is
            // closed too, as node closes a port whose partner closed.
            port.#closed = true;
            port.#remote = 0;
            stopWaiting(port);
            __kilnWorkerOp(OP_CLOSE, remote);
            closed(port);
          }
          return false;
        }
        let value: any;
        let ports: any[];
        try {
          [value, ports] = decodeRemote(got);
        } catch (error) {
          const target = port.#target === null ? port : port.#target;
          if (port.#kinds !== null) port.#kinds(-1, error, NO_PORTS);
          else dispatchTo(target, new MessageEvent("messageerror", { data: error }), listenersOf(target).get("messageerror"));
          return true;
        }
        if (port.#kinds !== null) {
          port.#kinds(got[0], value, ports);
          return true;
        }
        const target = port.#target === null ? port : port.#target;
        let list = port.#messageListeners;
        if (list === undefined) list = port.#messageListeners = listenersOf(target).get("message");
        dispatchTo(target, messageEvent(value, ports === NO_PORTS ? null : ports), list);
        if (list === undefined || list.length === 0) port.#autoRef();
        return true;
      };
      // Dispatch until the queue is empty, the budget is spent, or a checkpoint has something
      // to run; `budget` travels with the continuation so a batch stays one batch.
      const deliverSome = (port: MessagePort, budget: number) => {
        while (budget-- > 0) {
          const queue = port.#queue;
          const at = port.#head;
          if (at >= queue.length) {
            if (port.#remote === 0 || port.#closed) return;
            if (!deliverRemote(port)) {
              if (!port.#closed) listen(port);
              return;
            }
          } else {
            const data = queue[at];
            const ports = queue[at + 1];
            queue[at] = undefined;
            queue[at + 1] = undefined;
            if (at + 2 === queue.length) {
              queue.length = 0;
              port.#head = 0;
            } else {
              port.#head = at + 2;
            }
            if (port.#closed) return;
            if (data === CLOSE_MARK) {
              port.#closed = true;
              port.#queue = [];
              port.#head = 0;
              closed(port);
              return;
            }
            let list = port.#messageListeners;
            if (list === undefined) list = port.#messageListeners = listenersOf(port).get("message");
            dispatchTo(port, messageEvent(data, ports === NO_PORTS ? null : ports), list);
          }
          if (port.#closed) return;
          if (port.#head >= port.#queue.length && port.#remote === 0) return;
          const pending = __kilnJobsPending();
          if (pending !== 0) {
            if (nextTick === null) break;
            port.#scheduled = true;
            port.#budget = budget;
            continueAfter(port, pending);
            return;
          }
        }
        if (!port.#scheduled && !port.#closed && (port.#head < port.#queue.length || port.#remote !== 0)) {
          port.#scheduled = true;
          deliverLater(deliverNext, port);
        }
      };
      // Two ports continuing at once each see the other's continuation queued; after a bounded
      // number of hops a continuation stops waiting for the queue to empty and becomes a
      // macrotask instead, so neither can hold the other forever.
      const yielded = (port: MessagePort): boolean => {
        if (++port.#hops <= CONTINUATION_HOPS) return false;
        deliverLater(deliverNext, port);
        return true;
      };
      // Where a delivery goes on from when the checkpoint has `pending` work: behind the ticks,
      // behind the promise jobs, and once only a rejection nothing handled is left -- which the
      // loop reports when the checkpoint ends, and node reports before the next message -- from
      // the next turn's I/O callbacks.
      const continueAfter = (port: MessagePort, pending: number) => {
        if ((pending & TICKS) !== 0) (nextTick as any)(continueFromTick, port);
        else if ((pending & MICROTASKS) !== 0) queueMicrotask(() => continueFromMicrotask(port));
        else deliverLater(deliverNext, port);
      };
      continueFromTick = (port: MessagePort) => {
        const pending = __kilnJobsPending();
        if (pending !== 0) {
          if (!yielded(port)) continueAfter(port, pending);
          return;
        }
        port.#scheduled = false;
        port.#hops = 0;
        deliverSome(port, port.#budget);
      };
      continueFromMicrotask = (port: MessagePort) => {
        // The last hop is always a tick, so the next message's ticks run before its promise
        // jobs as they would after a node callback, rather than after the promise queue.
        if ((__kilnJobsPending() & MICROTASKS) !== 0) {
          if (yielded(port)) return;
          queueMicrotask(() => continueFromMicrotask(port));
          return;
        }
        (nextTick as any)(continueFromTick, port);
      };
      // Wait for the runtime to have something for a started port bound to another realm. The
      // wait is the realm's reason to stay alive while the port is referenced.
      listen = (port: MessagePort) => {
        if (port.#closed || port.#remote === 0 || port.#wait !== null || !port.#started) return;
        const pending = __kilnPortWait(port.#remote, !port.#unref);
        port.#wait = pending;
        pending.promise.then(() => {
          if (port.#wait !== pending) return;
          port.#wait = null;
          if (port.#closed || port.#scheduled) return;
          port.#scheduled = true;
          deliverLater(deliverNext, port);
        });
      };
      stopWaiting = (port: MessagePort) => {
        const pending = port.#wait;
        if (pending === null) return;
        port.#wait = null;
        __kilnWorkerOp(OP_CANCEL_WAIT, pending.requestId);
      };
      // In one realm a transferred port is a new object entangled with the old one's partner;
      // the old one is closed, and what was queued for it goes with the new one.
      transferPort = (port: MessagePort) => {
        const moved = new MessagePort();
        const other = port.#other;
        moved.#queue = port.#queue.slice(port.#head);
        moved.#remote = port.#remote;
        moved.#unref = port.#unref;
        port.#queue = [];
        port.#head = 0;
        port.#closed = true;
        port.#other = null;
        port.#remote = 0;
        stopWaiting(port);
        if (other !== null) {
          other.#other = moved;
          moved.#other = other;
        }
        return moved;
      };
      // A port leaving for another realm: the endpoint the other realm will bind. The port here
      // is closed without closing its channel; a partner still in this realm is bound to the
      // other end of a new channel, and what was queued here goes with the port.
      detachPort = (port: MessagePort): number => {
        let endpoint = port.#remote;
        if (endpoint === 0) {
          const pair = __kilnWorkerOp(OP_CHANNEL);
          endpoint = pair[1];
          const other = port.#other;
          if (other !== null && !other.#closed) {
            other.#other = null;
            other.#remote = pair[0];
            if (other.#started) listen(other);
          } else {
            __kilnWorkerOp(OP_CLOSE, pair[0]);
          }
          const queue = port.#queue;
          for (let at = port.#head; at < queue.length; at += 2) {
            if (queue[at] === CLOSE_MARK) break;
            __kilnWorkerOp(OP_ENQUEUE, endpoint, KIND_USER, encodeMessage(queue[at], queue[at + 1], null));
          }
        }
        port.#queue = [];
        port.#head = 0;
        port.#closed = true;
        port.#other = null;
        port.#remote = 0;
        stopWaiting(port);
        return endpoint;
      };
      remotePort = (endpoint: number): MessagePort => {
        const port = new MessagePort();
        port.#remote = endpoint;
        return port;
      };
      internalPort = (endpoint: number, kinds: any, target: any, broadcast: boolean): MessagePort => {
        const port = new MessagePort();
        port.#remote = endpoint;
        port.#kinds = kinds;
        port.#target = target;
        port.#broadcast = broadcast;
        return port;
      };
      portEndpoint = (port: MessagePort): number => port.#remote;
      kick = (port: MessagePort) => {
        if (port.#closed || port.#scheduled || port.#remote === 0 || !port.#started) return;
        port.#scheduled = true;
        deliverLater(deliverNext, port);
      };
      entangle = (a: MessagePort, b: MessagePort) => {
        a.#other = b;
        b.#other = a;
      };
    }
  }

  function closed(port: any): void {
    dispatchTo(port, new Event("close"), listenersOf(port).get("close"));
  }

  clone.registerHostKind((value: any) => value instanceof MessagePort
    ? "Object that needs transfer was found in message but not listed in transferList" : undefined);

  class MessageChannel {
    port1: MessagePort;
    port2: MessagePort;
    constructor() {
      this.port1 = new MessagePort();
      this.port2 = new MessagePort();
      entangle(this.port1, this.port2);
    }
    get [Symbol.toStringTag](): string { return "MessageChannel"; }
  }

  // A named group in the runtime, which every realm's channels of that name join: a post is
  // copied to each other member, in this realm or another, and delivered like a port's message.
  // An open channel keeps its realm alive, as node's does, until it is closed or unreferenced.
  // The channels of each name in this realm: a post reaches them through the runtime like any
  // other, and is delivered in the same turn's I/O callbacks, as node delivers it.
  const localChannels = new Map<string, Set<any>>();

  class BroadcastChannel extends EventTarget {
    #name: string;
    #closed = false;
    #port: MessagePort;
    #onmessage: Listener | null = null;
    #onmessageerror: Listener | null = null;
    constructor(name: any) {
      super();
      if (name === undefined && arguments.length === 0) throw new TypeError("The \"name\" argument must be specified");
      this.#name = String(name);
      this.#port = internalPort(__kilnWorkerOp(OP_BROADCAST_JOIN, this.#name), null, this, true);
      this.#port.start();
      let local = localChannels.get(this.#name);
      if (local === undefined) localChannels.set(this.#name, local = new Set());
      local.add(this);
    }
    get name(): string { return this.#name; }
    get onmessage(): any { return this.#onmessage === null ? null : this.#onmessage.callback; }
    set onmessage(handler: any) { this.#onmessage = setAttributeHandler(this, "message", handler, this.#onmessage); }
    get onmessageerror(): any { return this.#onmessageerror === null ? null : this.#onmessageerror.callback; }
    set onmessageerror(handler: any) { this.#onmessageerror = setAttributeHandler(this, "messageerror", handler, this.#onmessageerror); }
    postMessage(message: any): void {
      if (this.#closed) throw invalidState("BroadcastChannel is closed");
      if (message === undefined && arguments.length === 0) {
        throw typeError("The \"message\" argument must be specified", "ERR_MISSING_ARGS");
      }
      // Copied before anything else, so an uncloneable message throws with or without peers.
      this.#port.postMessage(message);
      const local = localChannels.get(this.#name);
      if (local !== undefined) for (const peer of local) if (peer !== this) kick(peer.#port);
    }
    close(): void {
      if (this.#closed) return;
      this.#closed = true;
      localChannels.get(this.#name)?.delete(this);
      const port = this.#port;
      // Closed quietly: the channel has no `close` event of its own.
      port.onmessage = null;
      port.close();
    }
    ref(): this { this.#port.ref(); return this; }
    unref(): this { this.#port.unref(); return this; }
    get [Symbol.toStringTag](): string { return "BroadcastChannel"; }
  }

  // ---- This realm as a worker (node:worker_threads) ----------------------------------------
  //
  // A worker's realm learns what it is from the runtime before any of its program runs: its
  // thread id, its end of the channel to its parent (`parentPort`), the endpoint `terminate()`
  // reaches it through, and what its parent sent it to start from. `node:worker_threads`
  // reads this; `process.env` is set here, before the program can read it.
  let workerState: any = undefined;
  (function adoptWorker() {
    const self = typeof __kilnWorkerOp === "function" ? __kilnWorkerOp(OP_SELF) : undefined;
    if (self === undefined) return;
    const [threadId, endpoint, control, info] = self;
    const parentPort = remotePort(endpoint);
    let workerData: any = null;
    let environmentData: any[] = [];
    const init = __kilnWorkerOp(OP_TAKE, endpoint);
    if (init !== undefined) {
      const [start] = decodeRemote(init);
      workerData = start[0];
      environmentData = start[2];
      const env = start[1];
      const proc = G.process;
      if (env !== null && env !== undefined && proc && proc.env && typeof proc.env === "object") {
        for (const key of Object.keys(proc.env)) if (!(key in env)) delete proc.env[key];
        for (const key of Object.keys(env)) proc.env[key] = env[key];
      }
    }
    // Stopping, when the parent asks: the runtime has raised the flag the engine checks, and
    // this ends the realm from wherever it was idle. Unreferenced: a worker with nothing else
    // to do ends on its own.
    const controlPort = internalPort(control, (kind: number) => {
      if (kind === KIND_TERMINATE && G.process && typeof G.process.exit === "function") G.process.exit(1);
    }, null, false);
    controlPort.unref();
    controlPort.start();
    // What a worker's `process` cannot do, since it would change the whole process: node refuses
    // each by name.
    if (G.process) {
      for (const name of ["chdir", "abort", "setuid", "setgid", "seteuid", "setegid", "setgroups", "initgroups"]) {
        Object.defineProperty(G.process, name, {
          value: function () {
            const error: any = new TypeError(`process.${name}() is not supported in workers`);
            error.code = "ERR_WORKER_UNSUPPORTED_OPERATION";
            throw error;
          },
          writable: true,
          enumerable: true,
          configurable: true,
        });
      }
    }
    // What the program writes to `process.stdout` and `process.stderr` goes to its parent, which
    // writes it to its own, as node pipes a worker's streams (`console.log` takes the engine's
    // own route there).
    const proc = G.process;
    if (proc) {
      for (const [name, stream] of [["stdout", 1], ["stderr", 2]] as [string, number][]) {
        const target = proc[name];
        if (target === null || typeof target !== "object") continue;
        target._write = (chunk: any, _encoding: any, callback: any) => {
          const text = typeof chunk === "string" ? chunk
            : typeof G.TextDecoder === "function" ? new G.TextDecoder().decode(chunk) : String(chunk);
          __kilnWorkerOp(OP_STDIO, stream, text);
          if (typeof callback === "function") callback();
        };
      }
    }
    workerState = {
      threadId,
      info: JSON.parse(info),
      parentPort,
      workerData,
      environmentData,
    };
  })();

  function cloneMessage(message: any): any {
    return clone.serialize(message, undefined);
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
  // An index loop without destructuring: `for (const [name, value] of classes)` made an iterator,
  // its results and one more iterator per pair in every realm's bootstrap (realms issue 08).
  const classDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  for (let i = 0; i < classes.length; i++) {
    const name = classes[i][0];
    const value = classes[i][1];
    // Own and hollow -- a constructor whose prototype lacks the members node's has -- counts
    // as absent: the interpreter's stubs were exactly that.
    const existing = G[name];
    const hollow = typeof existing === "function" && Object.getOwnPropertyNames(existing.prototype || {}).length <= 1;
    if (typeof existing === "undefined" || hollow) {
      classDescriptor.value = value;
      Object.defineProperty(G, name, classDescriptor);
    }
  }
})();
