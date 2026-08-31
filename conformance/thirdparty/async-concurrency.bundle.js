var __create = Object.create;
var __defProp = Object.defineProperty;
var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
var __getOwnPropNames = Object.getOwnPropertyNames;
var __getProtoOf = Object.getPrototypeOf;
var __hasOwnProp = Object.prototype.hasOwnProperty;
var __commonJS = (cb, mod) => function __require() {
  try {
    return mod || (0, cb[__getOwnPropNames(cb)[0]])((mod = { exports: {} }).exports, mod), mod.exports;
  } catch (e) {
    throw mod = 0, e;
  }
};
var __copyProps = (to, from2, except, desc) => {
  if (from2 && typeof from2 === "object" || typeof from2 === "function") {
    for (let key of __getOwnPropNames(from2))
      if (!__hasOwnProp.call(to, key) && key !== except)
        __defProp(to, key, { get: () => from2[key], enumerable: !(desc = __getOwnPropDesc(from2, key)) || desc.enumerable });
  }
  return to;
};
var __toESM = (mod, isNodeMode, target) => (target = mod != null ? __create(__getProtoOf(mod)) : {}, __copyProps(
  // If the importer is in node compatibility mode or this is not an ESM
  // file that has been converted to a CommonJS file using a Babel-
  // compatible transform (i.e. "__esModule" has not been set), then set
  // "default" to the CommonJS "module.exports" for node compatibility.
  isNodeMode || !mod || !mod.__esModule ? __defProp(target, "default", { value: mod, enumerable: true }) : target,
  mod
));

// node_modules/eventemitter3/index.js
var require_eventemitter3 = __commonJS({
  "node_modules/eventemitter3/index.js"(exports, module) {
    "use strict";
    var has = Object.prototype.hasOwnProperty;
    var prefix = "~";
    function Events() {
    }
    if (Object.create) {
      Events.prototype = /* @__PURE__ */ Object.create(null);
      if (!new Events().__proto__) prefix = false;
    }
    function EE(fn, context, once) {
      this.fn = fn;
      this.context = context;
      this.once = once || false;
    }
    function addListener(emitter, event, fn, context, once) {
      if (typeof fn !== "function") {
        throw new TypeError("The listener must be a function");
      }
      var listener = new EE(fn, context || emitter, once), evt = prefix ? prefix + event : event;
      if (!emitter._events[evt]) emitter._events[evt] = listener, emitter._eventsCount++;
      else if (!emitter._events[evt].fn) emitter._events[evt].push(listener);
      else emitter._events[evt] = [emitter._events[evt], listener];
      return emitter;
    }
    function clearEvent(emitter, evt) {
      if (--emitter._eventsCount === 0) emitter._events = new Events();
      else delete emitter._events[evt];
    }
    function EventEmitter2() {
      this._events = new Events();
      this._eventsCount = 0;
    }
    EventEmitter2.prototype.eventNames = function eventNames() {
      var names = [], events, name;
      if (this._eventsCount === 0) return names;
      for (name in events = this._events) {
        if (has.call(events, name)) names.push(prefix ? name.slice(1) : name);
      }
      if (Object.getOwnPropertySymbols) {
        return names.concat(Object.getOwnPropertySymbols(events));
      }
      return names;
    };
    EventEmitter2.prototype.listeners = function listeners(event) {
      var evt = prefix ? prefix + event : event, handlers = this._events[evt];
      if (!handlers) return [];
      if (handlers.fn) return [handlers.fn];
      for (var i = 0, l = handlers.length, ee = new Array(l); i < l; i++) {
        ee[i] = handlers[i].fn;
      }
      return ee;
    };
    EventEmitter2.prototype.listenerCount = function listenerCount(event) {
      var evt = prefix ? prefix + event : event, listeners = this._events[evt];
      if (!listeners) return 0;
      if (listeners.fn) return 1;
      return listeners.length;
    };
    EventEmitter2.prototype.emit = function emit(event, a1, a2, a3, a4, a5) {
      var evt = prefix ? prefix + event : event;
      if (!this._events[evt]) return false;
      var listeners = this._events[evt], len = arguments.length, args, i;
      if (listeners.fn) {
        if (listeners.once) this.removeListener(event, listeners.fn, void 0, true);
        switch (len) {
          case 1:
            return listeners.fn.call(listeners.context), true;
          case 2:
            return listeners.fn.call(listeners.context, a1), true;
          case 3:
            return listeners.fn.call(listeners.context, a1, a2), true;
          case 4:
            return listeners.fn.call(listeners.context, a1, a2, a3), true;
          case 5:
            return listeners.fn.call(listeners.context, a1, a2, a3, a4), true;
          case 6:
            return listeners.fn.call(listeners.context, a1, a2, a3, a4, a5), true;
        }
        for (i = 1, args = new Array(len - 1); i < len; i++) {
          args[i - 1] = arguments[i];
        }
        listeners.fn.apply(listeners.context, args);
      } else {
        var length = listeners.length, j;
        for (i = 0; i < length; i++) {
          if (listeners[i].once) this.removeListener(event, listeners[i].fn, void 0, true);
          switch (len) {
            case 1:
              listeners[i].fn.call(listeners[i].context);
              break;
            case 2:
              listeners[i].fn.call(listeners[i].context, a1);
              break;
            case 3:
              listeners[i].fn.call(listeners[i].context, a1, a2);
              break;
            case 4:
              listeners[i].fn.call(listeners[i].context, a1, a2, a3);
              break;
            default:
              if (!args) for (j = 1, args = new Array(len - 1); j < len; j++) {
                args[j - 1] = arguments[j];
              }
              listeners[i].fn.apply(listeners[i].context, args);
          }
        }
      }
      return true;
    };
    EventEmitter2.prototype.on = function on(event, fn, context) {
      return addListener(this, event, fn, context, false);
    };
    EventEmitter2.prototype.once = function once(event, fn, context) {
      return addListener(this, event, fn, context, true);
    };
    EventEmitter2.prototype.removeListener = function removeListener(event, fn, context, once) {
      var evt = prefix ? prefix + event : event;
      if (!this._events[evt]) return this;
      if (!fn) {
        clearEvent(this, evt);
        return this;
      }
      var listeners = this._events[evt];
      if (listeners.fn) {
        if (listeners.fn === fn && (!once || listeners.once) && (!context || listeners.context === context)) {
          clearEvent(this, evt);
        }
      } else {
        for (var i = 0, events = [], length = listeners.length; i < length; i++) {
          if (listeners[i].fn !== fn || once && !listeners[i].once || context && listeners[i].context !== context) {
            events.push(listeners[i]);
          }
        }
        if (events.length) this._events[evt] = events.length === 1 ? events[0] : events;
        else clearEvent(this, evt);
      }
      return this;
    };
    EventEmitter2.prototype.removeAllListeners = function removeAllListeners(event) {
      var evt;
      if (event) {
        evt = prefix ? prefix + event : event;
        if (this._events[evt]) clearEvent(this, evt);
      } else {
        this._events = new Events();
        this._eventsCount = 0;
      }
      return this;
    };
    EventEmitter2.prototype.off = EventEmitter2.prototype.removeListener;
    EventEmitter2.prototype.addListener = EventEmitter2.prototype.on;
    EventEmitter2.prefixed = prefix;
    EventEmitter2.EventEmitter = EventEmitter2;
    if ("undefined" !== typeof module) {
      module.exports = EventEmitter2;
    }
  }
});

// node_modules/eventemitter3/index.mjs
var import_index = __toESM(require_eventemitter3(), 1);

// node_modules/p-timeout/index.js
var TimeoutError = class _TimeoutError extends Error {
  name = "TimeoutError";
  constructor(message, options) {
    super(message, options);
    Error.captureStackTrace?.(this, _TimeoutError);
  }
};
var getAbortedReason = (signal) => signal.reason ?? new DOMException("This operation was aborted.", "AbortError");
function pTimeout(promise, options) {
  const {
    milliseconds,
    fallback,
    message,
    customTimers = { setTimeout, clearTimeout },
    signal
  } = options;
  let timer;
  let abortHandler;
  const wrappedPromise = new Promise((resolve, reject) => {
    if (typeof milliseconds !== "number" || Math.sign(milliseconds) !== 1) {
      throw new TypeError(`Expected \`milliseconds\` to be a positive number, got \`${milliseconds}\``);
    }
    if (signal?.aborted) {
      reject(getAbortedReason(signal));
      return;
    }
    if (signal) {
      abortHandler = () => {
        reject(getAbortedReason(signal));
      };
      signal.addEventListener("abort", abortHandler, { once: true });
    }
    promise.then(resolve, reject);
    if (milliseconds === Number.POSITIVE_INFINITY) {
      return;
    }
    const timeoutError = new TimeoutError();
    timer = customTimers.setTimeout.call(void 0, () => {
      if (fallback) {
        try {
          resolve(fallback());
        } catch (error) {
          reject(error);
        }
        return;
      }
      if (typeof promise.cancel === "function") {
        promise.cancel();
      }
      if (message === false) {
        resolve();
      } else if (message instanceof Error) {
        reject(message);
      } else {
        timeoutError.message = message ?? `Promise timed out after ${milliseconds} milliseconds`;
        reject(timeoutError);
      }
    }, milliseconds);
  });
  const cancelablePromise = wrappedPromise.finally(() => {
    cancelablePromise.clear();
    if (abortHandler && signal) {
      signal.removeEventListener("abort", abortHandler);
    }
  });
  cancelablePromise.clear = () => {
    customTimers.clearTimeout.call(void 0, timer);
    timer = void 0;
  };
  return cancelablePromise;
}

// node_modules/p-queue/dist/lower-bound.js
function lowerBound(array, value, comparator) {
  let first = 0;
  let count = array.length;
  while (count > 0) {
    const step2 = Math.trunc(count / 2);
    let it = first + step2;
    if (comparator(array[it], value) <= 0) {
      first = ++it;
      count -= step2 + 1;
    } else {
      count = step2;
    }
  }
  return first;
}

// node_modules/p-queue/dist/priority-queue.js
var compactionThreshold = 100;
var PriorityQueue = class {
  #queue = [];
  // The queue is stored as a sorted array, but dequeued items are left before `#head` until compaction. Only items from `#head` onward are live, which keeps repeated dequeues amortized O(1).
  #head = 0;
  enqueue(run, options) {
    const { priority = 0, id } = options ?? {};
    const { size } = this;
    const element = {
      priority,
      id,
      run
    };
    if (size === 0) {
      this.#queue.length = 0;
      this.#head = 0;
      this.#queue.push(element);
      return;
    }
    if (this.#queue.at(-1).priority >= priority) {
      this.#queue.push(element);
      return;
    }
    this.#compact();
    const index = lowerBound(this.#queue, element, (a, b) => b.priority - a.priority);
    this.#queue.splice(index, 0, element);
  }
  setPriority(id, priority) {
    const index = this.#queue.findIndex((element, index2) => index2 >= this.#head && element.id === id);
    if (index === -1) {
      throw new ReferenceError(`No promise function with the id "${id}" exists in the queue.`);
    }
    const [item] = this.#queue.splice(index, 1);
    this.enqueue(item.run, { priority, id });
  }
  remove(idOrRun) {
    const index = this.#queue.findIndex((element, index2) => {
      if (index2 < this.#head) {
        return false;
      }
      if (typeof idOrRun === "string") {
        return element.id === idOrRun;
      }
      return element.run === idOrRun;
    });
    if (index !== -1) {
      this.#queue.splice(index, 1);
    }
  }
  dequeue() {
    if (this.#head === this.#queue.length) {
      return void 0;
    }
    const item = this.#queue[this.#head];
    this.#head++;
    if (this.#head === this.#queue.length) {
      this.#queue.length = 0;
      this.#head = 0;
    } else if (this.#head > compactionThreshold && this.#head > this.#queue.length / 2) {
      this.#compact();
    }
    return item?.run;
  }
  filter(options) {
    const result = [];
    for (let index = this.#head; index < this.#queue.length; index++) {
      const element = this.#queue[index];
      if (element.priority === options.priority) {
        result.push(element.run);
      }
    }
    return result;
  }
  get size() {
    return this.#queue.length - this.#head;
  }
  #compact() {
    if (this.#head === 0) {
      return;
    }
    this.#queue.splice(0, this.#head);
    this.#head = 0;
  }
};

// node_modules/p-queue/dist/index.js
var PQueue = class extends import_index.default {
  #carryoverIntervalCount;
  #isIntervalIgnored;
  #intervalCount = 0;
  #intervalCap;
  #rateLimitedInInterval = false;
  #rateLimitFlushScheduled = false;
  #interval;
  /*
  Timestamp when the current interval window ends. Kept accurate at every window transition: `#initializeIntervalIfNeeded` sets it when a window starts and `#onInterval` advances it as the recurring timer rolls into each new window. This is the source of truth for whether a task added after the queue goes idle belongs to a fresh window, so it must not go stale.
  */
  #intervalEnd = 0;
  // Recurring timer that drives windows while the queue is actively processing. Absent (`undefined`) once the queue goes idle.
  #intervalId;
  // One-shot timer used while idle to resume at the next window boundary, since `#intervalId` is cleared when idle.
  #timeoutId;
  #strict;
  // Circular buffer implementation for better performance
  #strictTicks = [];
  #strictTicksStartIndex = 0;
  #queue;
  #queueClass;
  #pending = 0;
  // The `!` is needed because of https://github.com/microsoft/TypeScript/issues/32194
  #concurrency;
  #isPaused;
  // Use to assign a unique identifier to a promise function, if not explicitly specified
  #idAssigner = 1n;
  // Track currently running tasks for debugging
  #runningTasks = /* @__PURE__ */ new Map();
  #queueAbortListenerCleanupFunctions = /* @__PURE__ */ new Set();
  /**
      Get or set the default timeout for all tasks. Can be changed at runtime.
  
      Operations will throw a `TimeoutError` if they don't complete within the specified time.
  
      The timeout begins when the operation is dequeued and starts execution, not while it's waiting in the queue.
  
      @example
      ```
      const queue = new PQueue({timeout: 5000});
  
      // Change timeout for all future tasks
      queue.timeout = 10000;
      ```
      */
  timeout;
  constructor(options) {
    super();
    options = {
      carryoverIntervalCount: false,
      intervalCap: Number.POSITIVE_INFINITY,
      interval: 0,
      concurrency: Number.POSITIVE_INFINITY,
      autoStart: true,
      queueClass: PriorityQueue,
      strict: false,
      ...options
    };
    if (!(typeof options.intervalCap === "number" && options.intervalCap >= 1)) {
      throw new TypeError(`Expected \`intervalCap\` to be a number from 1 and up, got \`${options.intervalCap?.toString() ?? ""}\` (${typeof options.intervalCap})`);
    }
    if (options.interval === void 0 || !(Number.isFinite(options.interval) && options.interval >= 0)) {
      throw new TypeError(`Expected \`interval\` to be a finite number >= 0, got \`${options.interval?.toString() ?? ""}\` (${typeof options.interval})`);
    }
    if (options.strict && options.interval === 0) {
      throw new TypeError("The `strict` option requires a non-zero `interval`");
    }
    if (options.strict && options.intervalCap === Number.POSITIVE_INFINITY) {
      throw new TypeError("The `strict` option requires a finite `intervalCap`");
    }
    this.#carryoverIntervalCount = options.carryoverIntervalCount ?? options.carryoverConcurrencyCount ?? false;
    this.#isIntervalIgnored = options.intervalCap === Number.POSITIVE_INFINITY || options.interval === 0;
    this.#intervalCap = options.intervalCap;
    this.#interval = options.interval;
    this.#strict = options.strict;
    this.#queue = new options.queueClass();
    this.#queueClass = options.queueClass;
    this.concurrency = options.concurrency;
    if (options.timeout !== void 0 && !(Number.isFinite(options.timeout) && options.timeout > 0)) {
      throw new TypeError(`Expected \`timeout\` to be a positive finite number, got \`${options.timeout}\` (${typeof options.timeout})`);
    }
    this.timeout = options.timeout;
    this.#isPaused = options.autoStart === false;
    this.#setupRateLimitTracking();
  }
  #cleanupStrictTicks(now) {
    while (this.#strictTicksStartIndex < this.#strictTicks.length) {
      const oldestTick = this.#strictTicks[this.#strictTicksStartIndex];
      if (oldestTick !== void 0 && now - oldestTick >= this.#interval) {
        this.#strictTicksStartIndex++;
      } else {
        break;
      }
    }
    const shouldCompact = this.#strictTicksStartIndex > 100 && this.#strictTicksStartIndex > this.#strictTicks.length / 2 || this.#strictTicksStartIndex === this.#strictTicks.length;
    if (shouldCompact) {
      this.#strictTicks = this.#strictTicks.slice(this.#strictTicksStartIndex);
      this.#strictTicksStartIndex = 0;
    }
  }
  // Helper methods for interval consumption
  #consumeIntervalSlot(now) {
    if (this.#strict) {
      this.#strictTicks.push(now);
    } else {
      this.#intervalCount++;
    }
  }
  #rollbackIntervalSlot() {
    if (this.#strict) {
      if (this.#strictTicks.length > this.#strictTicksStartIndex) {
        this.#strictTicks.pop();
      }
    } else if (this.#intervalCount > 0) {
      this.#intervalCount--;
    }
  }
  #getActiveTicksCount() {
    return this.#strictTicks.length - this.#strictTicksStartIndex;
  }
  get #doesIntervalAllowAnother() {
    if (this.#isIntervalIgnored) {
      return true;
    }
    if (this.#strict) {
      return this.#getActiveTicksCount() < this.#intervalCap;
    }
    return this.#intervalCount < this.#intervalCap;
  }
  get #doesConcurrentAllowAnother() {
    return this.#pending < this.#concurrency;
  }
  #next() {
    this.#pending--;
    if (this.#pending === 0) {
      this.emit("pendingZero");
    }
    this.#tryToStartAnother();
    this.emit("next");
  }
  #onResumeInterval() {
    this.#timeoutId = void 0;
    this.#onInterval();
    this.#initializeIntervalIfNeeded();
  }
  #isIntervalPausedAt(now) {
    if (this.#strict) {
      this.#cleanupStrictTicks(now);
      const activeTicksCount = this.#getActiveTicksCount();
      if (activeTicksCount >= this.#intervalCap) {
        const oldestTick = this.#strictTicks[this.#strictTicksStartIndex];
        const delay = this.#interval - (now - oldestTick);
        this.#createIntervalTimeout(delay);
        return true;
      }
      return false;
    }
    if (this.#intervalId === void 0) {
      const delay = this.#intervalEnd - now;
      if (delay < 0) {
        this.#intervalCount = this.#carryoverIntervalCount ? this.#pending : 0;
      } else {
        this.#createIntervalTimeout(delay);
        return true;
      }
    }
    return false;
  }
  #createIntervalTimeout(delay) {
    if (this.#timeoutId !== void 0) {
      return;
    }
    this.#timeoutId = setTimeout(() => {
      this.#onResumeInterval();
    }, delay);
  }
  #clearIntervalTimer() {
    if (this.#intervalId) {
      clearInterval(this.#intervalId);
      this.#intervalId = void 0;
    }
  }
  #clearTimeoutTimer() {
    if (this.#timeoutId) {
      clearTimeout(this.#timeoutId);
      this.#timeoutId = void 0;
    }
  }
  #tryToStartAnother() {
    if (this.#queue.size === 0) {
      this.#clearIntervalTimer();
      this.emit("empty");
      if (this.#pending === 0) {
        this.#clearTimeoutTimer();
        if (this.#strict && this.#strictTicksStartIndex > 0) {
          const now = Date.now();
          this.#cleanupStrictTicks(now);
        }
        this.emit("idle");
      }
      return false;
    }
    let taskStarted = false;
    if (!this.#isPaused) {
      const now = Date.now();
      const canInitializeInterval = !this.#isIntervalPausedAt(now);
      if (this.#doesIntervalAllowAnother && this.#doesConcurrentAllowAnother) {
        const job = this.#queue.dequeue();
        if (!this.#isIntervalIgnored) {
          this.#consumeIntervalSlot(now);
          this.#scheduleRateLimitUpdate();
        }
        if (canInitializeInterval) {
          this.#initializeIntervalIfNeeded();
        }
        this.emit("active");
        job();
        taskStarted = true;
      }
    }
    return taskStarted;
  }
  #initializeIntervalIfNeeded() {
    if (this.#isIntervalIgnored || this.#intervalId !== void 0) {
      return;
    }
    if (this.#strict) {
      return;
    }
    this.#intervalId = setInterval(() => {
      this.#onInterval();
    }, this.#interval);
    this.#intervalEnd = Date.now() + this.#interval;
  }
  #onInterval() {
    if (!this.#strict) {
      if (this.#intervalId !== void 0) {
        if (this.#intervalCount === 0 && this.#pending === 0) {
          this.#clearIntervalTimer();
        } else {
          this.#intervalEnd = Date.now() + this.#interval;
        }
      }
      this.#intervalCount = this.#carryoverIntervalCount ? this.#pending : 0;
    }
    this.#processQueue();
    this.#scheduleRateLimitUpdate();
  }
  /**
  Executes all queued functions until it reaches the limit.
  */
  #processQueue() {
    while (this.#tryToStartAnother()) {
    }
  }
  get concurrency() {
    return this.#concurrency;
  }
  set concurrency(newConcurrency) {
    if (!(typeof newConcurrency === "number" && newConcurrency >= 1)) {
      throw new TypeError(`Expected \`concurrency\` to be a number from 1 and up, got \`${newConcurrency}\` (${typeof newConcurrency})`);
    }
    this.#concurrency = newConcurrency;
    this.#processQueue();
  }
  /**
      Updates the priority of a promise function by its id, affecting its execution order. Requires a defined concurrency limit to take effect.
  
      For example, this can be used to prioritize a promise function to run earlier.
  
      ```js
      import PQueue from 'p-queue';
  
      const queue = new PQueue({concurrency: 1});
  
      queue.add(async () => '🦄', {priority: 1});
      queue.add(async () => '🦀', {priority: 0, id: '🦀'});
      queue.add(async () => '🦄', {priority: 1});
      queue.add(async () => '🦄', {priority: 1});
  
      queue.setPriority('🦀', 2);
      ```
  
      In this case, the promise function with `id: '🦀'` runs second.
  
      You can also deprioritize a promise function to delay its execution:
  
      ```js
      import PQueue from 'p-queue';
  
      const queue = new PQueue({concurrency: 1});
  
      queue.add(async () => '🦄', {priority: 1});
      queue.add(async () => '🦀', {priority: 1, id: '🦀'});
      queue.add(async () => '🦄');
      queue.add(async () => '🦄', {priority: 0});
  
      queue.setPriority('🦀', -1);
      ```
      Here, the promise function with `id: '🦀'` executes last.
      */
  setPriority(id, priority) {
    if (typeof priority !== "number" || !Number.isFinite(priority)) {
      throw new TypeError(`Expected \`priority\` to be a finite number, got \`${priority}\` (${typeof priority})`);
    }
    this.#queue.setPriority(id, priority);
  }
  async add(function_, options = {}) {
    options = {
      timeout: this.timeout,
      ...options,
      // Assign unique ID if not provided
      id: options.id ?? (this.#idAssigner++).toString()
    };
    if (options.timeout !== void 0 && !(Number.isFinite(options.timeout) && options.timeout > 0)) {
      throw new TypeError(`Expected \`timeout\` to be a positive finite number, got \`${options.timeout}\` (${typeof options.timeout})`);
    }
    return new Promise((resolve, reject) => {
      const taskSymbol = /* @__PURE__ */ Symbol(`task-${options.id}`);
      let cleanupQueueAbortHandler = () => void 0;
      const run = async () => {
        cleanupQueueAbortHandler();
        this.#pending++;
        this.#runningTasks.set(taskSymbol, {
          id: options.id,
          priority: options.priority ?? 0,
          // Match priority-queue default
          startTime: Date.now(),
          timeout: options.timeout
        });
        let eventListener;
        try {
          try {
            options.signal?.throwIfAborted();
          } catch (error) {
            this.#rollbackIntervalConsumption();
            this.#runningTasks.delete(taskSymbol);
            throw error;
          }
          let operation = function_({ signal: options.signal });
          if (options.timeout !== void 0) {
            operation = pTimeout(Promise.resolve(operation), {
              milliseconds: options.timeout,
              message: `Task timed out after ${options.timeout}ms (queue has ${this.#pending} running, ${this.#queue.size} waiting)`
            });
          }
          if (options.signal) {
            const { signal } = options;
            operation = Promise.race([operation, new Promise((_resolve, reject2) => {
              eventListener = () => {
                reject2(signal.reason);
              };
              signal.addEventListener("abort", eventListener, { once: true });
            })]);
          }
          const result = await operation;
          resolve(result);
          this.emit("completed", result);
        } catch (error) {
          reject(error);
          this.emit("error", error);
        } finally {
          if (eventListener) {
            options.signal?.removeEventListener("abort", eventListener);
          }
          this.#runningTasks.delete(taskSymbol);
          queueMicrotask(() => {
            this.#next();
          });
        }
      };
      this.#queue.enqueue(run, options);
      const removeQueuedTask = () => {
        if (this.#queue instanceof PriorityQueue) {
          this.#queue.remove(run);
          return;
        }
        this.#queue.remove?.(options.id);
      };
      if (options.signal) {
        const { signal } = options;
        const queueAbortHandler = () => {
          cleanupQueueAbortHandler();
          removeQueuedTask();
          reject(signal.reason);
          this.#tryToStartAnother();
          this.emit("next");
        };
        cleanupQueueAbortHandler = () => {
          signal.removeEventListener("abort", queueAbortHandler);
          this.#queueAbortListenerCleanupFunctions.delete(cleanupQueueAbortHandler);
        };
        if (signal.aborted) {
          queueAbortHandler();
          return;
        }
        signal.addEventListener("abort", queueAbortHandler, { once: true });
        this.#queueAbortListenerCleanupFunctions.add(cleanupQueueAbortHandler);
      }
      this.emit("add");
      this.#tryToStartAnother();
    });
  }
  async addAll(functions, options) {
    return Promise.all(functions.map(async (function_) => this.add(function_, options)));
  }
  /**
  Start (or resume) executing enqueued tasks within concurrency limit. No need to call this if queue is not paused (via `options.autoStart = false` or by `.pause()` method.)
  */
  start() {
    if (!this.#isPaused) {
      return this;
    }
    this.#isPaused = false;
    this.#processQueue();
    return this;
  }
  /**
  Put queue execution on hold.
  */
  pause() {
    this.#isPaused = true;
  }
  /**
  Clear the queue.
  */
  clear() {
    for (const cleanupQueueAbortHandler of this.#queueAbortListenerCleanupFunctions) {
      cleanupQueueAbortHandler();
    }
    this.#queue = new this.#queueClass();
    this.#clearIntervalTimer();
    this.#updateRateLimitState();
    this.emit("empty");
    if (this.#pending === 0) {
      this.#clearTimeoutTimer();
      this.emit("idle");
    }
    this.emit("next");
  }
  /**
      Can be called multiple times. Useful if you for example add additional items at a later time.
  
      @returns A promise that settles when the queue becomes empty.
      */
  async onEmpty() {
    if (this.#queue.size === 0) {
      return;
    }
    await this.#onEvent("empty");
  }
  /**
      @returns A promise that settles when the queue size is less than the given limit: `queue.size < limit`.
  
      If you want to avoid having the queue grow beyond a certain size you can `await queue.onSizeLessThan()` before adding a new item.
  
      Note that this only limits the number of items waiting to start. There could still be up to `concurrency` jobs already running that this call does not include in its calculation.
      */
  async onSizeLessThan(limit) {
    if (this.#queue.size < limit) {
      return;
    }
    await this.#onEvent(["next", "active"], () => this.#queue.size < limit);
  }
  /**
      The difference with `.onEmpty` is that `.onIdle` guarantees that all work from the queue has finished. `.onEmpty` merely signals that the queue is empty, but it could mean that some promises haven't completed yet.
  
      @returns A promise that settles when the queue becomes empty, and all promises have completed; `queue.size === 0 && queue.pending === 0`.
      */
  async onIdle() {
    if (this.#pending === 0 && this.#queue.size === 0) {
      return;
    }
    await this.#onEvent("idle");
  }
  /**
      The difference with `.onIdle` is that `.onPendingZero` only waits for currently running tasks to finish, ignoring queued tasks.
  
      @returns A promise that settles when all currently running tasks have completed; `queue.pending === 0`.
      */
  async onPendingZero() {
    if (this.#pending === 0) {
      return;
    }
    await this.#onEvent("pendingZero");
  }
  /**
  @returns A promise that settles when the queue becomes rate-limited due to intervalCap.
  */
  async onRateLimit() {
    if (this.isRateLimited) {
      return;
    }
    await this.#onEvent("rateLimit");
  }
  /**
  @returns A promise that settles when the queue is no longer rate-limited.
  */
  async onRateLimitCleared() {
    if (!this.isRateLimited) {
      return;
    }
    await this.#onEvent("rateLimitCleared");
  }
  /**
      @returns A promise that rejects when any task in the queue errors.
  
      Use with `Promise.race([queue.onError(), queue.onIdle()])` to fail fast on the first error while still resolving normally when the queue goes idle.
  
      Important: The promise returned by `add()` still rejects. You must handle each `add()` promise (for example, `.catch(() => {})`) to avoid unhandled rejections.
  
      @example
      ```
      import PQueue from 'p-queue';
  
      const queue = new PQueue({concurrency: 2});
  
      queue.add(() => fetchData(1)).catch(() => {});
      queue.add(() => fetchData(2)).catch(() => {});
      queue.add(() => fetchData(3)).catch(() => {});
  
      // Stop processing on first error
      try {
          await Promise.race([
              queue.onError(),
              queue.onIdle()
          ]);
      } catch (error) {
          queue.pause(); // Stop processing remaining tasks
          console.error('Queue failed:', error);
      }
      ```
      */
  // eslint-disable-next-line @typescript-eslint/promise-function-async
  onError() {
    return new Promise((_resolve, reject) => {
      const handleError = (error) => {
        this.off("error", handleError);
        reject(error);
      };
      this.on("error", handleError);
    });
  }
  async #onEvent(events, filter2) {
    const eventList = Array.isArray(events) ? events : [events];
    return new Promise((resolve) => {
      const listener = () => {
        if (filter2 && !filter2()) {
          return;
        }
        for (const event of eventList) {
          this.off(event, listener);
        }
        resolve();
      };
      for (const event of eventList) {
        this.on(event, listener);
      }
    });
  }
  /**
  Size of the queue, the number of queued items waiting to run.
  */
  get size() {
    return this.#queue.size;
  }
  /**
      Size of the queue, filtered by the given options.
  
      For example, this can be used to find the number of items remaining in the queue with a specific priority level.
      */
  sizeBy(options) {
    return this.#queue.filter(options).length;
  }
  /**
  Number of running items (no longer in the queue).
  */
  get pending() {
    return this.#pending;
  }
  /**
  Whether the queue is currently paused.
  */
  get isPaused() {
    return this.#isPaused;
  }
  #setupRateLimitTracking() {
    if (this.#isIntervalIgnored) {
      return;
    }
    this.on("add", () => {
      if (this.#queue.size > 0) {
        this.#scheduleRateLimitUpdate();
      }
    });
    this.on("next", () => {
      this.#scheduleRateLimitUpdate();
    });
  }
  #scheduleRateLimitUpdate() {
    if (this.#isIntervalIgnored || this.#rateLimitFlushScheduled) {
      return;
    }
    this.#rateLimitFlushScheduled = true;
    queueMicrotask(() => {
      this.#rateLimitFlushScheduled = false;
      this.#updateRateLimitState();
    });
  }
  #rollbackIntervalConsumption() {
    if (this.#isIntervalIgnored) {
      return;
    }
    this.#rollbackIntervalSlot();
    this.#scheduleRateLimitUpdate();
  }
  #updateRateLimitState() {
    const previous = this.#rateLimitedInInterval;
    if (this.#isIntervalIgnored || this.#queue.size === 0) {
      if (previous) {
        this.#rateLimitedInInterval = false;
        this.emit("rateLimitCleared");
      }
      return;
    }
    let count;
    if (this.#strict) {
      const now = Date.now();
      this.#cleanupStrictTicks(now);
      count = this.#getActiveTicksCount();
    } else {
      count = this.#intervalCount;
    }
    const shouldBeRateLimited = count >= this.#intervalCap;
    if (shouldBeRateLimited !== previous) {
      this.#rateLimitedInInterval = shouldBeRateLimited;
      this.emit(shouldBeRateLimited ? "rateLimit" : "rateLimitCleared");
    }
  }
  /**
  Whether the queue is currently rate-limited due to intervalCap.
  */
  get isRateLimited() {
    return this.#rateLimitedInInterval;
  }
  /**
      Whether the queue is saturated. Returns `true` when:
      - All concurrency slots are occupied and tasks are waiting, OR
      - The queue is rate-limited and tasks are waiting
  
      Useful for detecting backpressure and potential hanging tasks.
  
      ```js
      import PQueue from 'p-queue';
  
      const queue = new PQueue({concurrency: 2});
  
      // Backpressure handling
      if (queue.isSaturated) {
          console.log('Queue is saturated, waiting for capacity...');
          await queue.onSizeLessThan(queue.concurrency);
      }
  
      // Monitoring for stuck tasks
      setInterval(() => {
          if (queue.isSaturated) {
              console.warn(`Queue saturated: ${queue.pending} running, ${queue.size} waiting`);
          }
      }, 60000);
      ```
      */
  get isSaturated() {
    return this.#pending === this.#concurrency && this.#queue.size > 0 || this.isRateLimited && this.#queue.size > 0;
  }
  /**
      The tasks currently being executed. Each task includes its `id`, `priority`, `startTime`, `timeout` (if set), and `timeoutRemaining` (milliseconds until the task times out, or `undefined` if no timeout is set).
  
      Returns an array of task info objects.
  
      ```js
      import PQueue from 'p-queue';
  
      const queue = new PQueue({concurrency: 2, timeout: 10000});
  
      // Add tasks with IDs for better debugging
      queue.add(() => fetchUser(123), {id: 'user-123'});
      queue.add(() => fetchPosts(456), {id: 'posts-456', priority: 1});
  
      // Check what's running
      console.log(queue.runningTasks);
      // => [{
      //   id: 'user-123',
      //   priority: 0,
      //   startTime: 1759253001716,
      //   timeout: 10000,
      //   timeoutRemaining: 9700
      // }, {
      //   id: 'posts-456',
      //   priority: 1,
      //   startTime: 1759253001916,
      //   timeout: 10000,
      //   timeoutRemaining: 9900
      // }]
      ```
      */
  get runningTasks() {
    return [...this.#runningTasks.values()].map((task) => ({
      ...task,
      timeoutRemaining: task.timeout ? Math.max(0, task.startTime + task.timeout - Date.now()) : void 0
    }));
  }
};

// node_modules/p-map/index.js
async function pMap(iterable, mapper, {
  concurrency = Number.POSITIVE_INFINITY,
  stopOnError = true,
  signal
} = {}) {
  return new Promise((resolve_, reject_) => {
    if (iterable[Symbol.iterator] === void 0 && iterable[Symbol.asyncIterator] === void 0) {
      throw new TypeError(`Expected \`input\` to be either an \`Iterable\` or \`AsyncIterable\`, got (${typeof iterable})`);
    }
    if (typeof mapper !== "function") {
      throw new TypeError("Mapper function is required");
    }
    if (!(Number.isSafeInteger(concurrency) && concurrency >= 1 || concurrency === Number.POSITIVE_INFINITY)) {
      throw new TypeError(`Expected \`concurrency\` to be an integer from 1 and up or \`Infinity\`, got \`${concurrency}\` (${typeof concurrency})`);
    }
    const result = [];
    const errors = [];
    const skippedIndexesMap = /* @__PURE__ */ new Map();
    let isRejected = false;
    let isResolved = false;
    let isIterableDone = false;
    let resolvingCount = 0;
    let currentIndex = 0;
    const iterator = iterable[Symbol.iterator] === void 0 ? iterable[Symbol.asyncIterator]() : iterable[Symbol.iterator]();
    const signalListener = () => {
      reject(signal.reason);
    };
    const cleanup = () => {
      signal?.removeEventListener("abort", signalListener);
    };
    const resolve = (value) => {
      resolve_(value);
      cleanup();
    };
    const reject = (reason) => {
      isRejected = true;
      isResolved = true;
      reject_(reason);
      cleanup();
    };
    if (signal) {
      if (signal.aborted) {
        reject(signal.reason);
        return;
      }
      signal.addEventListener("abort", signalListener, { once: true });
    }
    const next = async () => {
      if (isResolved) {
        return;
      }
      const nextItem = await iterator.next();
      const index = currentIndex;
      currentIndex++;
      if (nextItem.done) {
        isIterableDone = true;
        if (resolvingCount === 0 && !isResolved) {
          if (!stopOnError && errors.length > 0) {
            reject(new AggregateError(errors));
            return;
          }
          isResolved = true;
          if (skippedIndexesMap.size === 0) {
            resolve(result);
            return;
          }
          const pureResult = [];
          for (const [index2, value] of result.entries()) {
            if (skippedIndexesMap.get(index2) === pMapSkip) {
              continue;
            }
            pureResult.push(value);
          }
          resolve(pureResult);
        }
        return;
      }
      resolvingCount++;
      (async () => {
        try {
          const element = await nextItem.value;
          if (isResolved) {
            return;
          }
          const value = await mapper(element, index);
          if (value === pMapSkip) {
            skippedIndexesMap.set(index, value);
          }
          result[index] = value;
          resolvingCount--;
          await next();
        } catch (error) {
          if (stopOnError) {
            reject(error);
          } else {
            errors.push(error);
            resolvingCount--;
            try {
              await next();
            } catch (error2) {
              reject(error2);
            }
          }
        }
      })();
    };
    (async () => {
      for (let index = 0; index < concurrency; index++) {
        try {
          await next();
        } catch (error) {
          reject(error);
          break;
        }
        if (isIterableDone || isRejected) {
          break;
        }
      }
    })();
  });
}
var pMapSkip = /* @__PURE__ */ Symbol("skip");

// node_modules/yocto-queue/index.js
var Node = class {
  value;
  next;
  constructor(value) {
    this.value = value;
  }
};
var Queue = class {
  #head;
  #tail;
  #size;
  constructor() {
    this.clear();
  }
  enqueue(value) {
    const node = new Node(value);
    if (this.#head) {
      this.#tail.next = node;
      this.#tail = node;
    } else {
      this.#head = node;
      this.#tail = node;
    }
    this.#size++;
  }
  dequeue() {
    const current = this.#head;
    if (!current) {
      return;
    }
    this.#head = this.#head.next;
    this.#size--;
    if (!this.#head) {
      this.#tail = void 0;
    }
    return current.value;
  }
  peek() {
    if (!this.#head) {
      return;
    }
    return this.#head.value;
  }
  clear() {
    this.#head = void 0;
    this.#tail = void 0;
    this.#size = 0;
  }
  get size() {
    return this.#size;
  }
  *[Symbol.iterator]() {
    let current = this.#head;
    while (current) {
      yield current.value;
      current = current.next;
    }
  }
  *drain() {
    while (this.#head) {
      yield this.dequeue();
    }
  }
};

// node_modules/p-limit/index.js
function pLimit(concurrency) {
  let rejectOnClear = false;
  if (typeof concurrency === "object") {
    ({ concurrency, rejectOnClear = false } = concurrency);
  }
  validateConcurrency(concurrency);
  if (typeof rejectOnClear !== "boolean") {
    throw new TypeError("Expected `rejectOnClear` to be a boolean");
  }
  const queue = new Queue();
  let activeCount = 0;
  const resumeNext = () => {
    if (activeCount < concurrency && queue.size > 0) {
      activeCount++;
      queue.dequeue().run();
    }
  };
  const next = () => {
    activeCount--;
    resumeNext();
  };
  const run = async (function_, resolve, arguments_) => {
    const result = (async () => function_(...arguments_))();
    resolve(result);
    try {
      await result;
    } catch {
    }
    next();
  };
  const enqueue = (function_, resolve, reject, arguments_) => {
    const queueItem = { reject };
    new Promise((internalResolve) => {
      queueItem.run = internalResolve;
      queue.enqueue(queueItem);
    }).then(run.bind(void 0, function_, resolve, arguments_));
    if (activeCount < concurrency) {
      resumeNext();
    }
  };
  const generator = (function_, ...arguments_) => new Promise((resolve, reject) => {
    enqueue(function_, resolve, reject, arguments_);
  });
  Object.defineProperties(generator, {
    activeCount: {
      get: () => activeCount
    },
    pendingCount: {
      get: () => queue.size
    },
    clearQueue: {
      value() {
        if (!rejectOnClear) {
          queue.clear();
          return;
        }
        const abortError = AbortSignal.abort().reason;
        while (queue.size > 0) {
          queue.dequeue().reject(abortError);
        }
      }
    },
    concurrency: {
      get: () => concurrency,
      set(newConcurrency) {
        validateConcurrency(newConcurrency);
        concurrency = newConcurrency;
        queueMicrotask(() => {
          while (activeCount < concurrency && queue.size > 0) {
            resumeNext();
          }
        });
      }
    },
    map: {
      async value(iterable, function_) {
        const promises = Array.from(iterable, (value, index) => generator(function_, value, index));
        return Promise.all(promises);
      }
    }
  });
  return generator;
}
function validateConcurrency(concurrency) {
  if (!((Number.isInteger(concurrency) || concurrency === Number.POSITIVE_INFINITY) && concurrency > 0)) {
    throw new TypeError("Expected `concurrency` to be a number from 1 and up");
  }
}

// node_modules/is-network-error/index.js
var objectToString = Object.prototype.toString;
var isError = (value) => objectToString.call(value) === "[object Error]";
var errorMessages = /* @__PURE__ */ new Set([
  "network error",
  // Chrome
  "NetworkError when attempting to fetch resource.",
  // Firefox
  "The Internet connection appears to be offline.",
  // Safari 16
  "Network request failed",
  // `cross-fetch`
  "fetch failed",
  // Undici (Node.js)
  "terminated",
  // Undici (Node.js)
  " A network error occurred.",
  // Bun (WebKit)
  "Network connection lost"
  // Cloudflare Workers (fetch)
]);
function isNetworkError(error) {
  const isValid = error && isError(error) && error.name === "TypeError" && typeof error.message === "string";
  if (!isValid) {
    return false;
  }
  const { message, stack } = error;
  if (message === "Load failed" || message.startsWith("Load failed (") && message.endsWith(")")) {
    return stack === void 0 || "__sentry_captured__" in error;
  }
  if (message.startsWith("error sending request for url")) {
    return true;
  }
  if (message === "Failed to fetch" || message.startsWith("Failed to fetch (") && message.endsWith(")")) {
    return true;
  }
  return errorMessages.has(message);
}

// node_modules/p-retry/index.js
function validateRetries(retries) {
  if (typeof retries === "number") {
    if (retries < 0) {
      throw new TypeError("Expected `retries` to be a non-negative number.");
    }
    if (Number.isNaN(retries)) {
      throw new TypeError("Expected `retries` to be a valid number or Infinity, got NaN.");
    }
  } else if (retries !== void 0) {
    throw new TypeError("Expected `retries` to be a number or Infinity.");
  }
}
function validateNumberOption(name, value, { min = 0, allowInfinity = false } = {}) {
  if (value === void 0) {
    return;
  }
  if (typeof value !== "number" || Number.isNaN(value)) {
    throw new TypeError(`Expected \`${name}\` to be a number${allowInfinity ? " or Infinity" : ""}.`);
  }
  if (!allowInfinity && !Number.isFinite(value)) {
    throw new TypeError(`Expected \`${name}\` to be a finite number.`);
  }
  if (value < min) {
    throw new TypeError(`Expected \`${name}\` to be \u2265 ${min}.`);
  }
}
function validateFunctionOption(name, value) {
  if (value === void 0) {
    return;
  }
  if (typeof value !== "function") {
    throw new TypeError(`Expected \`${name}\` to be a function.`);
  }
}
var AbortError = class extends Error {
  constructor(message) {
    super();
    if (message instanceof Error) {
      this.originalError = message;
      ({ message } = message);
    } else {
      this.originalError = new Error(message);
      this.originalError.stack = this.stack;
    }
    this.name = "AbortError";
    this.message = message;
  }
};
function calculateDelay(retriesConsumed, options) {
  const attempt = Math.max(1, retriesConsumed + 1);
  const random = options.randomize ? Math.random() + 1 : 1;
  let timeout = Math.round(random * options.minTimeout * options.factor ** (attempt - 1));
  timeout = Math.min(timeout, options.maxTimeout);
  return timeout;
}
function calculateRemainingTime(start, max) {
  if (!Number.isFinite(max)) {
    return max;
  }
  return max - (performance.now() - start);
}
async function delayForRetry(delay, options) {
  if (delay <= 0) {
    return;
  }
  await new Promise((resolve, reject) => {
    const onAbort = () => {
      clearTimeout(timeoutToken);
      options.signal?.removeEventListener("abort", onAbort);
      reject(options.signal.reason);
    };
    const timeoutToken = setTimeout(() => {
      options.signal?.removeEventListener("abort", onAbort);
      resolve();
    }, delay);
    if (options.unref) {
      timeoutToken.unref?.();
    }
    options.signal?.addEventListener("abort", onAbort, { once: true });
  });
}
async function onAttemptFailure({ error, attemptNumber, retriesConsumed, startTime, options }) {
  const normalizedError = error instanceof Error ? error : new TypeError(`Non-error was thrown: "${error}". You should only throw errors.`);
  if (normalizedError instanceof AbortError) {
    throw normalizedError.originalError;
  }
  const retriesLeft = Number.isFinite(options.retries) ? Math.max(0, options.retries - retriesConsumed) : options.retries;
  const maxRetryTime = options.maxRetryTime ?? Number.POSITIVE_INFINITY;
  const delayTime = calculateDelay(retriesConsumed, options);
  const remainingTimeBeforeCallbacks = calculateRemainingTime(startTime, maxRetryTime);
  if (remainingTimeBeforeCallbacks <= 0) {
    const context2 = Object.freeze({
      error: normalizedError,
      attemptNumber,
      retriesLeft,
      retriesConsumed,
      retryDelay: 0
    });
    await options.onFailedAttempt(context2);
    throw normalizedError;
  }
  const consumeRetryContext = Object.freeze({
    error: normalizedError,
    attemptNumber,
    retriesLeft,
    retriesConsumed,
    retryDelay: retriesLeft > 0 ? delayTime : 0
  });
  const consumeRetry = await options.shouldConsumeRetry(consumeRetryContext);
  const effectiveDelay = consumeRetry && retriesLeft > 0 ? delayTime : 0;
  const context = Object.freeze({
    error: normalizedError,
    attemptNumber,
    retriesLeft,
    retriesConsumed,
    retryDelay: effectiveDelay
  });
  await options.onFailedAttempt(context);
  if (calculateRemainingTime(startTime, maxRetryTime) <= 0) {
    throw normalizedError;
  }
  const remainingTime = calculateRemainingTime(startTime, maxRetryTime);
  if (remainingTime <= 0 || retriesLeft <= 0) {
    throw normalizedError;
  }
  if (normalizedError instanceof TypeError && !isNetworkError(normalizedError)) {
    throw normalizedError;
  }
  if (!await options.shouldRetry(context)) {
    throw normalizedError;
  }
  const remainingTimeAfterShouldRetry = calculateRemainingTime(startTime, maxRetryTime);
  if (remainingTimeAfterShouldRetry <= 0) {
    throw normalizedError;
  }
  if (!consumeRetry) {
    options.signal?.throwIfAborted();
    return false;
  }
  const finalDelay = Math.min(effectiveDelay, remainingTimeAfterShouldRetry);
  options.signal?.throwIfAborted();
  await delayForRetry(finalDelay, options);
  options.signal?.throwIfAborted();
  return true;
}
async function pRetry(input, options = {}) {
  options = { ...options };
  validateRetries(options.retries);
  if (Object.hasOwn(options, "forever")) {
    throw new Error("The `forever` option is no longer supported. For many use-cases, you can set `retries: Infinity` instead.");
  }
  options.retries ??= 10;
  options.factor ??= 2;
  options.minTimeout ??= 1e3;
  options.maxTimeout ??= Number.POSITIVE_INFINITY;
  options.maxRetryTime ??= Number.POSITIVE_INFINITY;
  options.randomize ??= false;
  options.onFailedAttempt ??= () => {
  };
  options.shouldRetry ??= () => true;
  options.shouldConsumeRetry ??= () => true;
  validateFunctionOption("onFailedAttempt", options.onFailedAttempt);
  validateFunctionOption("shouldRetry", options.shouldRetry);
  validateFunctionOption("shouldConsumeRetry", options.shouldConsumeRetry);
  validateNumberOption("factor", options.factor, { min: 0, allowInfinity: false });
  validateNumberOption("minTimeout", options.minTimeout, { min: 0, allowInfinity: false });
  validateNumberOption("maxTimeout", options.maxTimeout, { min: 0, allowInfinity: true });
  validateNumberOption("maxRetryTime", options.maxRetryTime, { min: 0, allowInfinity: true });
  if (!(options.factor > 0)) {
    options.factor = 1;
  }
  options.signal?.throwIfAborted();
  let attemptNumber = 0;
  let retriesConsumed = 0;
  const startTime = performance.now();
  while (Number.isFinite(options.retries) ? retriesConsumed <= options.retries : true) {
    attemptNumber++;
    try {
      options.signal?.throwIfAborted();
      const result = await input(attemptNumber);
      options.signal?.throwIfAborted();
      return result;
    } catch (error) {
      if (await onAttemptFailure({
        error,
        attemptNumber,
        retriesConsumed,
        startTime,
        options
      })) {
        retriesConsumed++;
      }
    }
  }
  throw new Error("Retry attempts exhausted without throwing an error.");
}

// node_modules/tslib/tslib.es6.mjs
function __awaiter(thisArg, _arguments, P, generator) {
  function adopt(value) {
    return value instanceof P ? value : new P(function(resolve) {
      resolve(value);
    });
  }
  return new (P || (P = Promise))(function(resolve, reject) {
    function fulfilled(value) {
      try {
        step2(generator.next(value));
      } catch (e) {
        reject(e);
      }
    }
    function rejected(value) {
      try {
        step2(generator["throw"](value));
      } catch (e) {
        reject(e);
      }
    }
    function step2(result) {
      result.done ? resolve(result.value) : adopt(result.value).then(fulfilled, rejected);
    }
    step2((generator = generator.apply(thisArg, _arguments || [])).next());
  });
}
function __values(o) {
  var s = typeof Symbol === "function" && Symbol.iterator, m = s && o[s], i = 0;
  if (m) return m.call(o);
  if (o && typeof o.length === "number") return {
    next: function() {
      if (o && i >= o.length) o = void 0;
      return { value: o && o[i++], done: !o };
    }
  };
  throw new TypeError(s ? "Object is not iterable." : "Symbol.iterator is not defined.");
}
function __await(v) {
  return this instanceof __await ? (this.v = v, this) : new __await(v);
}
function __asyncGenerator(thisArg, _arguments, generator) {
  if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
  var g = generator.apply(thisArg, _arguments || []), i, q = [];
  return i = Object.create((typeof AsyncIterator === "function" ? AsyncIterator : Object).prototype), verb("next"), verb("throw"), verb("return", awaitReturn), i[Symbol.asyncIterator] = function() {
    return this;
  }, i;
  function awaitReturn(f) {
    return function(v) {
      return Promise.resolve(v).then(f, reject);
    };
  }
  function verb(n, f) {
    if (g[n]) {
      i[n] = function(v) {
        return new Promise(function(a, b) {
          q.push([n, v, a, b]) > 1 || resume(n, v);
        });
      };
      if (f) i[n] = f(i[n]);
    }
  }
  function resume(n, v) {
    try {
      step2(g[n](v));
    } catch (e) {
      settle(q[0][3], e);
    }
  }
  function step2(r) {
    r.value instanceof __await ? Promise.resolve(r.value.v).then(fulfill, reject) : settle(q[0][2], r);
  }
  function fulfill(value) {
    resume("next", value);
  }
  function reject(value) {
    resume("throw", value);
  }
  function settle(f, v) {
    if (f(v), q.shift(), q.length) resume(q[0][0], q[0][1]);
  }
}
function __asyncValues(o) {
  if (!Symbol.asyncIterator) throw new TypeError("Symbol.asyncIterator is not defined.");
  var m = o[Symbol.asyncIterator], i;
  return m ? m.call(o) : (o = typeof __values === "function" ? __values(o) : o[Symbol.iterator](), i = {}, verb("next"), verb("throw"), verb("return"), i[Symbol.asyncIterator] = function() {
    return this;
  }, i);
  function verb(n) {
    i[n] = o[n] && function(v) {
      return new Promise(function(resolve, reject) {
        v = o[n](v), settle(resolve, reject, v.done, v.value);
      });
    };
  }
  function settle(resolve, reject, d, v) {
    Promise.resolve(v).then(function(v2) {
      resolve({ value: v2, done: d });
    }, reject);
  }
}

// node_modules/ix/util/bindcallback.mjs
function bindCallback(func, thisArg, argCount) {
  if (typeof thisArg === "undefined") {
    return func;
  }
  switch (argCount) {
    case 0:
      return function() {
        return func.call(thisArg);
      };
    case 1:
      return function(arg) {
        return func.call(thisArg, arg);
      };
    case 2:
      return function(value, index) {
        return func.call(thisArg, value, index);
      };
    case 3:
      return function(value, index, collection) {
        return func.call(thisArg, value, index, collection);
      };
    default:
      return function() {
        return func.apply(thisArg, arguments);
      };
  }
}

// node_modules/ix/util/identity.mjs
function identityAsync(x) {
  return __awaiter(this, void 0, void 0, function* () {
    return x;
  });
}

// node_modules/ix/util/isiterable.mjs
var isNumber = (x) => typeof x === "number";
var isBoolean = (x) => typeof x === "boolean";
var isFunction = (x) => typeof x === "function";
var isObject = (x) => x != null && Object(x) === x;
var isPromise = (x) => {
  return isObject(x) && isFunction(x.then);
};
function isArrayLike(x) {
  return isObject(x) && isNumber(x["length"]);
}
function isIterable(x) {
  return x != null && isFunction(x[Symbol.iterator]);
}
function isIterator(x) {
  return isObject(x) && !isFunction(x[Symbol.iterator]) && isFunction(x["next"]);
}
function isAsyncIterable(x) {
  return isObject(x) && isFunction(x[Symbol.asyncIterator]);
}
function isObservable(x) {
  return x != null && Object(x) === x && typeof x["subscribe"] === "function";
}
var isReadableNodeStream = (x) => {
  return isObject(x) && isFunction(x["pipe"]) && isFunction(x["_read"]) && isBoolean(x["readable"]) && isObject(x["_readableState"]);
};
var isWritableNodeStream = (x) => {
  return isObject(x) && isFunction(x["end"]) && isFunction(x["_write"]) && isBoolean(x["writable"]) && isObject(x["_writableState"]);
};

// node_modules/ix/util/tointeger.mjs
function toInteger(value) {
  const number = Number(value);
  if (isNaN(number)) {
    return 0;
  }
  if (number === 0 || !isFinite(number)) {
    return number;
  }
  return (number > 0 ? 1 : -1) * Math.floor(Math.abs(number));
}

// node_modules/ix/util/tolength.mjs
var maxSafeInteger = Math.pow(2, 53) - 1;
function toLength(value) {
  const len = toInteger(value);
  return Math.min(Math.max(len, 0), maxSafeInteger);
}

// node_modules/ix/aborterror.mjs
var AbortError2 = class _AbortError extends Error {
  constructor(message = "The operation has been aborted") {
    super(message);
    Object.setPrototypeOf(this, _AbortError.prototype);
    Error.captureStackTrace(this, this.constructor);
    this.name = "AbortError";
  }
  get [Symbol.toStringTag]() {
    return "AbortError";
  }
};
function throwIfAborted(signal) {
  if (signal && signal.aborted) {
    throw new AbortError2();
  }
}
Object.defineProperty(AbortError2, Symbol.hasInstance, {
  writable: true,
  configurable: true,
  value(x) {
    return isObject(x) && (x.constructor.name === "AbortError" || x[Symbol.toStringTag] === "AbortError");
  }
});

// node_modules/ix/asynciterable/asynciterablex.mjs
var AsyncIterableX = class _AsyncIterableX {
  /** @nocollapse */
  forEach(projection, thisArg, signal) {
    var _a, e_1, _b, _c;
    return __awaiter(this, void 0, void 0, function* () {
      const source = signal ? new WithAbortAsyncIterable(this, signal) : this;
      let i = 0;
      try {
        for (var _d = true, source_1 = __asyncValues(source), source_1_1; source_1_1 = yield source_1.next(), _a = source_1_1.done, !_a; _d = true) {
          _c = source_1_1.value;
          _d = false;
          const item = _c;
          yield projection.call(thisArg, item, i++, signal);
        }
      } catch (e_1_1) {
        e_1 = { error: e_1_1 };
      } finally {
        try {
          if (!_d && !_a && (_b = source_1.return)) yield _b.call(source_1);
        } finally {
          if (e_1) throw e_1.error;
        }
      }
    });
  }
  pipe(...args) {
    let i = -1;
    const n = args.length;
    let acc = this;
    while (++i < n) {
      acc = args[i](_AsyncIterableX.as(acc));
    }
    return acc;
  }
  /** @nocollapse */
  static from(source, selector = identityAsync, thisArg) {
    const fn = bindCallback(selector, thisArg, 2);
    if (isIterable(source) || isAsyncIterable(source)) {
      return new FromAsyncIterable(source, fn);
    }
    if (isPromise(source)) {
      return new FromPromiseIterable(source, fn);
    }
    if (isObservable(source)) {
      return new FromObservableAsyncIterable(source, fn);
    }
    if (isArrayLike(source)) {
      return new FromArrayIterable(source, fn);
    }
    if (isIterator(source)) {
      return new FromAsyncIterable({ [Symbol.asyncIterator]: () => source }, fn);
    }
    throw new TypeError("Input type not supported");
  }
  /**
   * Converts the input into an async-iterable sequence.
   *
   * @param {*} source The source to convert to an async-iterable sequence.
   * @returns {AsyncIterableX<*>} An async-iterable containing the input.
   */
  /** @nocollapse */
  static as(source) {
    if (source instanceof _AsyncIterableX) {
      return source;
    }
    if (typeof source === "string") {
      return new FromArrayIterable([source], identityAsync);
    }
    if (isIterable(source) || isAsyncIterable(source)) {
      return new FromAsyncIterable(source, identityAsync);
    }
    if (isPromise(source)) {
      return new FromPromiseIterable(source, identityAsync);
    }
    if (isObservable(source)) {
      return new FromObservableAsyncIterable(source, identityAsync);
    }
    if (isArrayLike(source)) {
      return new FromArrayIterable(source, identityAsync);
    }
    return new FromArrayIterable([source], identityAsync);
  }
};
AsyncIterableX.prototype[Symbol.toStringTag] = "AsyncIterableX";
Object.defineProperty(AsyncIterableX, Symbol.hasInstance, {
  writable: true,
  configurable: true,
  value(inst) {
    return !!(inst && inst[Symbol.toStringTag] === "AsyncIterableX");
  }
});
var ARRAY_VALUE = "value";
var ARRAY_ERROR = "error";
var AsyncSink = class {
  constructor() {
    this._ended = false;
    this._values = [];
    this._resolvers = [];
  }
  [Symbol.asyncIterator]() {
    return this;
  }
  write(value) {
    this._push({ type: ARRAY_VALUE, value });
  }
  error(error) {
    this._push({ type: ARRAY_ERROR, error });
  }
  _push(item) {
    if (this._ended) {
      throw new Error("AsyncSink already ended");
    }
    if (this._resolvers.length > 0) {
      const { resolve, reject } = this._resolvers.shift();
      if (item.type === ARRAY_ERROR) {
        reject(item.error);
      } else {
        resolve({ done: false, value: item.value });
      }
    } else {
      this._values.push(item);
    }
  }
  next() {
    if (this._values.length > 0) {
      const { type, value, error } = this._values.shift();
      if (type === ARRAY_ERROR) {
        return Promise.reject(error);
      } else {
        return Promise.resolve({ done: false, value });
      }
    }
    if (this._ended) {
      return Promise.resolve({ done: true });
    }
    return new Promise((resolve, reject) => {
      this._resolvers.push({ resolve, reject });
    });
  }
  end() {
    while (this._resolvers.length > 0) {
      this._resolvers.shift().resolve({ done: true });
    }
    this._ended = true;
  }
};
var FromArrayIterable = class extends AsyncIterableX {
  constructor(source, selector) {
    super();
    this._source = source;
    this._selector = selector;
  }
  [Symbol.asyncIterator]() {
    return __asyncGenerator(this, arguments, function* _a() {
      let i = 0;
      const length = toLength(this._source.length);
      while (i < length) {
        yield yield __await(yield __await(this._selector(this._source[i], i++)));
      }
    });
  }
};
var FromAsyncIterable = class extends AsyncIterableX {
  constructor(source, selector) {
    super();
    this._source = source;
    this._selector = selector;
  }
  [Symbol.asyncIterator](signal) {
    return __asyncGenerator(this, arguments, function* _a() {
      var _b, e_2, _c, _d, _e, e_3, _f, _g;
      let i = 0;
      if (signal && this._source instanceof AsyncIterableX) {
        try {
          for (var _h = true, _j = __asyncValues(new WithAbortAsyncIterable(this._source, signal)), _k; _k = yield __await(_j.next()), _b = _k.done, !_b; _h = true) {
            _d = _k.value;
            _h = false;
            const item = _d;
            yield yield __await(yield __await(this._selector(item, i++)));
          }
        } catch (e_2_1) {
          e_2 = { error: e_2_1 };
        } finally {
          try {
            if (!_h && !_b && (_c = _j.return)) yield __await(_c.call(_j));
          } finally {
            if (e_2) throw e_2.error;
          }
        }
      } else {
        throwIfAborted(signal);
        try {
          for (var _l = true, _m = __asyncValues(this._source), _o; _o = yield __await(_m.next()), _e = _o.done, !_e; _l = true) {
            _g = _o.value;
            _l = false;
            const item = _g;
            throwIfAborted(signal);
            const value = yield __await(this._selector(item, i++));
            throwIfAborted(signal);
            yield yield __await(value);
          }
        } catch (e_3_1) {
          e_3 = { error: e_3_1 };
        } finally {
          try {
            if (!_l && !_e && (_f = _m.return)) yield __await(_f.call(_m));
          } finally {
            if (e_3) throw e_3.error;
          }
        }
      }
    });
  }
};
var FromPromiseIterable = class extends AsyncIterableX {
  constructor(source, selector) {
    super();
    this._source = source;
    this._selector = selector;
  }
  [Symbol.asyncIterator]() {
    return __asyncGenerator(this, arguments, function* _a() {
      const item = yield __await(this._source);
      yield yield __await(yield __await(this._selector(item, 0)));
    });
  }
};
var FromObservableAsyncIterable = class extends AsyncIterableX {
  constructor(observable, selector) {
    super();
    this._observable = observable;
    this._selector = selector;
  }
  [Symbol.asyncIterator](signal) {
    return __asyncGenerator(this, arguments, function* _a() {
      throwIfAborted(signal);
      const sink = new AsyncSink();
      const subscription = this._observable.subscribe({
        next(value) {
          sink.write(value);
        },
        error(err) {
          sink.error(err);
        },
        complete() {
          sink.end();
        }
      });
      function onAbort() {
        sink.error(new AbortError2());
      }
      if (signal) {
        signal.addEventListener("abort", onAbort);
      }
      let i = 0;
      try {
        for (let next; !(next = yield __await(sink.next())).done; ) {
          throwIfAborted(signal);
          yield yield __await(yield __await(this._selector(next.value, i++)));
        }
      } finally {
        if (signal) {
          signal.removeEventListener("abort", onAbort);
        }
        subscription.unsubscribe();
      }
    });
  }
};
var WithAbortAsyncIterable = class {
  constructor(source, signal) {
    this._source = source;
    this._signal = signal;
  }
  [Symbol.asyncIterator]() {
    return this._source[Symbol.asyncIterator](this._signal);
  }
};
try {
  ((isBrowser) => {
    if (isBrowser) {
      return;
    }
    AsyncIterableX.prototype["pipe"] = nodePipe;
    const readableOpts = (x, opts = x._writableState || { objectMode: true }) => opts;
    function nodePipe(...args) {
      let i = -1;
      let end;
      const n = args.length;
      let prev = this;
      let next;
      while (++i < n) {
        next = args[i];
        if (typeof next === "function") {
          prev = next(AsyncIterableX.as(prev));
        } else if (isWritableNodeStream(next)) {
          ({ end = true } = args[i + 1] || {});
          return isReadableNodeStream(prev) ? prev.pipe(next, { end }) : AsyncIterableX.as(prev).toNodeStream(readableOpts(next)).pipe(next, { end });
        }
      }
      return prev;
    }
  })(typeof window === "object" && typeof document === "object" && document.nodeType === 9);
} catch (e) {
}
var as = AsyncIterableX.as;
var from = AsyncIterableX.from;

// node_modules/ix/asynciterable/operators/withabort.mjs
var WithAbortAsyncIterable2 = class _WithAbortAsyncIterable extends AsyncIterableX {
  constructor(source, signal) {
    super();
    this._source = source;
    this._signal = signal;
  }
  withAbort(signal) {
    return new _WithAbortAsyncIterable(this._source, signal);
  }
  [Symbol.asyncIterator]() {
    return this._source[Symbol.asyncIterator](this._signal);
  }
};
function wrapWithAbort(source, signal) {
  return signal ? new WithAbortAsyncIterable2(source, signal) : source;
}

// node_modules/ix/util/returniterator.mjs
function returnAsyncIterator(it) {
  return __awaiter(this, void 0, void 0, function* () {
    if (typeof (it === null || it === void 0 ? void 0 : it.return) === "function") {
      yield it.return();
    }
  });
}

// node_modules/ix/util/safeRace.mjs
function isPrimitive(value) {
  return value === null || typeof value !== "object" && typeof value !== "function";
}
var wm = /* @__PURE__ */ new WeakMap();
function safeRace(contenders) {
  let deferred;
  const result = new Promise((resolve, reject) => {
    deferred = { resolve, reject };
    for (const contender of contenders) {
      if (isPrimitive(contender)) {
        Promise.resolve(contender).then(resolve, reject);
        continue;
      }
      let record = wm.get(contender);
      if (record === void 0) {
        record = { deferreds: /* @__PURE__ */ new Set([deferred]), settled: false };
        wm.set(contender, record);
        Promise.resolve(contender).then((value) => {
          for (const { resolve: resolve2 } of record.deferreds) {
            resolve2(value);
          }
          record.deferreds.clear();
          record.settled = true;
        }, (err) => {
          for (const { reject: reject2 } of record.deferreds) {
            reject2(err);
          }
          record.deferreds.clear();
          record.settled = true;
        });
      } else if (record.settled) {
        Promise.resolve(contender).then(resolve, reject);
      } else {
        record.deferreds.add(deferred);
      }
    }
  });
  return result.finally(() => {
    for (const contender of contenders) {
      if (!isPrimitive(contender)) {
        const record = wm.get(contender);
        record.deferreds.delete(deferred);
      }
    }
  });
}

// node_modules/ix/asynciterable/operators/map.mjs
var MapAsyncIterable = class extends AsyncIterableX {
  constructor(source, selector, thisArg) {
    super();
    this._source = source;
    this._selector = selector;
    this._thisArg = thisArg;
  }
  [Symbol.asyncIterator](signal) {
    return __asyncGenerator(this, arguments, function* _a() {
      var _b, e_1, _c, _d;
      throwIfAborted(signal);
      let i = 0;
      try {
        for (var _e = true, _f = __asyncValues(wrapWithAbort(this._source, signal)), _g; _g = yield __await(_f.next()), _b = _g.done, !_b; _e = true) {
          _d = _g.value;
          _e = false;
          const item = _d;
          const result = yield __await(this._selector.call(this._thisArg, item, i++, signal));
          yield yield __await(result);
        }
      } catch (e_1_1) {
        e_1 = { error: e_1_1 };
      } finally {
        try {
          if (!_e && !_b && (_c = _f.return)) yield __await(_c.call(_f));
        } finally {
          if (e_1) throw e_1.error;
        }
      }
    });
  }
};
function map(selector, thisArg) {
  return function mapOperatorFunction(source) {
    return new MapAsyncIterable(source, selector, thisArg);
  };
}

// node_modules/ix/asynciterable/operators/filter.mjs
var FilterAsyncIterable = class extends AsyncIterableX {
  constructor(source, predicate, thisArg) {
    super();
    this._source = source;
    this._predicate = predicate;
    this._thisArg = thisArg;
  }
  [Symbol.asyncIterator](signal) {
    return __asyncGenerator(this, arguments, function* _a() {
      var _b, e_1, _c, _d;
      throwIfAborted(signal);
      let i = 0;
      try {
        for (var _e = true, _f = __asyncValues(wrapWithAbort(this._source, signal)), _g; _g = yield __await(_f.next()), _b = _g.done, !_b; _e = true) {
          _d = _g.value;
          _e = false;
          const item = _d;
          if (yield __await(this._predicate.call(this._thisArg, item, i++))) {
            yield yield __await(item);
          }
        }
      } catch (e_1_1) {
        e_1 = { error: e_1_1 };
      } finally {
        try {
          if (!_e && !_b && (_c = _f.return)) yield __await(_c.call(_f));
        } finally {
          if (e_1) throw e_1.error;
        }
      }
    });
  }
};
function filter(predicate, thisArg) {
  return function filterOperatorFunction(source) {
    return new FilterAsyncIterable(source, predicate, thisArg);
  };
}

// node_modules/ix/asynciterable/operators/_flatten.mjs
var NEVER_PROMISE = new Promise(() => {
});
function ignoreInnerAbortErrors(signal) {
  return function ignoreInnerAbortError(e) {
    if (signal.aborted && e instanceof AbortError2) {
      return NEVER_PROMISE;
    }
    throw e;
  };
}
function wrapIterator(source, index, type, signal) {
  return __asyncGenerator(this, arguments, function* wrapIterator_1() {
    var _a, e_1, _b, _c;
    throwIfAborted(signal);
    try {
      for (var _d = true, _e = __asyncValues(wrapWithAbort(source, signal)), _f; _f = yield __await(_e.next()), _a = _f.done, !_a; _d = true) {
        _c = _f.value;
        _d = false;
        const value = _c;
        throwIfAborted(signal);
        yield yield __await({ type, index, value });
      }
    } catch (e_1_1) {
      e_1 = { error: e_1_1 };
    } finally {
      try {
        if (!_d && !_a && (_b = _e.return)) yield __await(_b.call(_e));
      } finally {
        if (e_1) throw e_1.error;
      }
    }
    return yield __await({ type, index, value: void 0 });
  });
}
var FlattenConcurrentAsyncIterable = class extends AsyncIterableX {
  constructor(_source, _selector, _concurrent, _switchMode, _thisArg) {
    super();
    this._source = _source;
    this._selector = _selector;
    this._concurrent = _concurrent;
    this._switchMode = _switchMode;
    this._thisArg = _thisArg;
    this._concurrent = this._switchMode ? 1 : Math.max(_concurrent, 1);
  }
  [Symbol.asyncIterator](outerSignal) {
    return __asyncGenerator(this, arguments, function* _a() {
      throwIfAborted(outerSignal);
      let active = 0;
      let outerIndex = 0;
      let outerComplete = false;
      const thisArg = this._thisArg;
      const selector = this._selector;
      const switchMode = this._switchMode;
      const concurrent = this._concurrent;
      const outerValues = new Array(0);
      const innerIndices = new Array(0);
      const controllers = new Array(isFinite(concurrent) ? concurrent : 0);
      const inners = new Array(isFinite(concurrent) ? concurrent : 0);
      const outer = wrapIterator(this._source, 0, 0, outerSignal);
      const results = [outer.next()];
      try {
        do {
          const { done = false, value: { type, value, index } } = yield __await(safeRace(results));
          if (!done) {
            switch (type) {
              case 0: {
                if (switchMode) {
                  active = 0;
                }
                if (active < concurrent) {
                  pullNextOuter(value);
                } else {
                  outerValues.push(value);
                }
                results[0] = outer.next();
                break;
              }
              case 1: {
                yield yield __await(value);
                const { [index - 1]: inner } = inners;
                const { [index - 1]: { signal } } = controllers;
                results[index] = inner.next().catch(ignoreInnerAbortErrors(signal));
                break;
              }
            }
          } else {
            results[index] = NEVER_PROMISE;
            switch (type) {
              case 0: {
                outerComplete = true;
                break;
              }
              case 1: {
                --active;
                innerIndices.push(index);
                while (active < concurrent && outerValues.length) {
                  pullNextOuter(outerValues.shift());
                }
                break;
              }
            }
          }
        } while (!outerComplete || active + outerValues.length > 0);
      } finally {
        controllers.forEach((controller) => controller === null || controller === void 0 ? void 0 : controller.abort());
        yield __await(Promise.all([outer, ...inners].map(returnAsyncIterator)));
      }
      function pullNextOuter(outerValue) {
        ++active;
        const index = innerIndices.pop() || active;
        if (switchMode && controllers[index - 1]) {
          controllers[index - 1].abort();
        }
        controllers[index - 1] = new AbortController();
        const innerSignal = controllers[index - 1].signal;
        const inner = selector.call(thisArg, outerValue, outerIndex++, innerSignal);
        results[index] = wrapAndPullInner(index, innerSignal, inner).catch(ignoreInnerAbortErrors(innerSignal));
      }
      function wrapAndPullInner(index, signal, inner) {
        if (isPromise(inner)) {
          return inner.then((inner2) => wrapAndPullInner(index, signal, inner2));
        }
        return (inners[index - 1] = wrapIterator(AsyncIterableX.as(inner), index, 1, signal)).next();
      }
    });
  }
};

// node_modules/ix/asynciterable/operators/flatmap.mjs
function flatMap(selector, concurrent = Infinity, thisArg) {
  return function flatMapOperatorFunction(source) {
    return new FlattenConcurrentAsyncIterable(source, selector, concurrent, false, thisArg);
  };
}

// async-concurrency.entry.mjs
var lines = [];
var say = (...a) => lines.push(a.join(" "));
var tick = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms));
async function step(name, fn) {
  try {
    await fn();
  } catch (e) {
    say(name, "THREW", e && (e.constructor ? e.constructor.name : "?"), e && e.message);
  }
}
async function main() {
  await step("p-limit", async () => {
    const limit = pLimit(2);
    let live = 0, peak = 0;
    const limited = await Promise.all([5, 4, 3, 2, 1].map((n) => limit(async () => {
      live++;
      peak = Math.max(peak, live);
      await tick(n);
      live--;
      return n;
    })));
    say("p-limit", limited.join(","), "peak<=2", peak <= 2);
  });
  await step("p-map-array", async () => {
    say("p-map-array", (await pMap([1, 2, 3, 4], async (x) => x * 2, { concurrency: 2 })).join(","));
    async function* gen() {
      for (const x of [1, 2, 3]) {
        await Promise.resolve();
        yield x;
      }
    }
    say("p-map-asynciter", (await pMap(gen(), async (x) => x + 10, { concurrency: 2 })).join(","));
  });
  await step("p-queue", async () => {
    const q = new PQueue({ concurrency: 1 });
    const order = [];
    q.add(async () => {
      order.push("a");
      await tick(2);
    }, { priority: 0 });
    q.add(async () => {
      order.push("b");
    }, { priority: 10 });
    q.add(async () => {
      order.push("c");
    }, { priority: 5 });
    await q.onIdle();
    say("p-queue", order.join(""), "size", q.size, "pending", q.pending);
  });
  await step("p-retry", async () => {
    let n = 0;
    say("p-retry", await pRetry(async () => {
      n++;
      if (n < 3) throw new Error("no");
      return "ok@" + n;
    }, { retries: 5, minTimeout: 1 }));
    try {
      await pRetry(async () => {
        throw new AbortError("stop");
      }, { retries: 5, minTimeout: 1 });
      say("p-retry-abort", "no-throw");
    } catch (e) {
      say("p-retry-abort", e.message);
    }
  });
  await step("p-timeout-ok", async () => {
    say("p-timeout-ok", await pTimeout(tick(1, "fast"), { milliseconds: 200 }));
    try {
      await pTimeout(tick(200, "slow"), { milliseconds: 5 });
      say("p-timeout-late", "no-throw");
    } catch (e) {
      say("p-timeout-late", e.name);
    }
  });
  await step("abort", async () => {
    const ac = new AbortController();
    const aborted = [];
    ac.signal.addEventListener("abort", () => aborted.push("evt:" + ac.signal.reason));
    ac.abort("why");
    say("abort", ac.signal.aborted, aborted.join(""), typeof AbortSignal.timeout);
  });
  await step("allSettled", async () => {
    const settled = await Promise.allSettled([Promise.resolve(1), Promise.reject(new Error("e")), tick(1, 3)]);
    say("allSettled", settled.map((s) => s.status + ":" + (s.status === "fulfilled" ? s.value : s.reason.message)).join(","));
    say("any", await Promise.any([Promise.reject(new Error("a")), tick(1, "b")]));
    try {
      await Promise.any([Promise.reject(new Error("x")), Promise.reject(new Error("y"))]);
      say("any-all-reject", "no-throw");
    } catch (e) {
      say("any-all-reject", e.constructor.name, e.errors.map((x) => x.message).join(""));
    }
    say("race", await Promise.race([tick(50, "slow"), tick(1, "fast")]));
  });
  await step("withResolvers", async () => {
    if (typeof Promise.withResolvers === "function") {
      const { promise, resolve } = Promise.withResolvers();
      resolve("wr");
      say("withResolvers", await promise);
    } else {
      say("withResolvers", "unsupported");
    }
    class Tracked extends Promise {
      static tally = 0;
      constructor(exec) {
        super(exec);
        Tracked.tally++;
      }
    }
    const t = Tracked.resolve(1).then((x) => x + 1);
    say("subclass", await t, t instanceof Tracked, Tracked.tally > 0, Tracked[Symbol.species] === Tracked);
  });
  await step("ordering", async () => {
    const seq = [];
    await new Promise((resolve) => {
      setTimeout(() => {
        seq.push("timeout");
        resolve();
      }, 0);
      queueMicrotask(() => seq.push("micro"));
      Promise.resolve().then(() => seq.push("then"));
    });
    say("ordering", seq.join(">"));
  });
  await step("ix", async () => {
    const ix = from([1, 2, 3, 4, 5]).pipe(map(async (x) => x * 2), filter((x) => x > 4), flatMap((x) => from([x, -x])));
    const got = [];
    for await (const v of ix) {
      got.push(v);
      if (got.length === 4) break;
    }
    say("ix", got.join(","), ix instanceof AsyncIterableX);
  });
  await step("for-await-sync", async () => {
    const awaited = [];
    for await (const v of [Promise.resolve("p1"), tick(1, "p2"), "p3"]) awaited.push(v);
    say("for-await-sync", awaited.join(","));
  });
  await step("asyncgen-finally", async () => {
    const closed = [];
    async function* withFinally() {
      try {
        yield 1;
        yield 2;
      } finally {
        closed.push("cleanup");
      }
    }
    for await (const v of withFinally()) {
      closed.push("v" + v);
      break;
    }
    say("asyncgen-finally", closed.join(","));
  });
  console.log(lines.join("\n"));
}
main().catch((e) => {
  console.log("FATAL", e && (e.stack || e.message));
});
