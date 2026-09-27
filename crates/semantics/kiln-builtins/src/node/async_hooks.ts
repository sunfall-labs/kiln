// kiln-builtins: node:async_hooks implementation (Phase 5.2, ADR 0005)

// AsyncLocalStorage and AsyncResource have language-level implementations in this
// module. A private host capability may replace them, but the public builtin
// loader must never recursively ask for this module while it is evaluating.
const nativeModule =
  typeof process !== "undefined"
    ? (process as any).__kilnNativeModules?.async_hooks
    : null;

const storageMap = new WeakMap<object, any>();
const disabledSet = new WeakSet<object>();

function FallbackAsyncLocalStorage(this: any) {
  if (!(this instanceof FallbackAsyncLocalStorage)) {
    throw new TypeError("Class constructor AsyncLocalStorage cannot be invoked without 'new'");
  }
  storageMap.set(this, undefined);
  this.run = function run(store: any, callback: (...args: any[]) => any, ...args: any[]) {
    if (typeof callback !== "function") {
      throw new TypeError("AsyncLocalStorage callback is not a function");
    }
    const displaced = storageMap.get(this);
    storageMap.set(this, store);
    try {
      return callback(...args);
    } finally {
      storageMap.set(this, displaced);
    }
  };
  this.getStore = function getStore() {
    if (disabledSet.has(this)) return undefined;
    return storageMap.get(this);
  };
  this.enterWith = function enterWith(store: any) {
    storageMap.set(this, store);
  };
  this.exit = function exit(callback: (...args: any[]) => any, ...args: any[]) {
    if (typeof callback !== "function") {
      throw new TypeError("AsyncLocalStorage.exit requires a callback");
    }
    const displaced = storageMap.get(this);
    storageMap.delete(this);
    try {
      return callback(...args);
    } finally {
      storageMap.set(this, displaced);
    }
  };
  this.disable = function disable() {
    disabledSet.add(this);
    storageMap.delete(this);
  };
}

function FallbackAsyncResource(this: any, _type?: string, _triggerAsyncId?: any) {
  if (!(this instanceof FallbackAsyncResource)) {
    throw new TypeError("Class constructor AsyncResource cannot be invoked without 'new'");
  }
  this.bind = function bind(fn: (...args: any[]) => any, thisArg?: any) {
    if (typeof fn !== "function") {
      throw new TypeError("The \"fn\" argument must be of type function");
    }
    const targetThis = thisArg !== undefined ? thisArg : this;
    return function (this: any, ...args: any[]) {
      return fn.apply(targetThis, args);
    };
  };
  this.emitDestroy = function emitDestroy() {
    return this;
  };
}
FallbackAsyncResource.bind = function bind(fn: (...args: any[]) => any, _type?: string, thisArg?: any) {
  if (typeof fn !== "function") {
    throw new TypeError("The \"fn\" argument must be of type function");
  }
  return function (this: any, ...args: any[]) {
    return fn.apply(thisArg !== undefined ? thisArg : this, args);
  };
};

export const AsyncLocalStorage: any =
  nativeModule && typeof nativeModule.AsyncLocalStorage === "function"
    ? nativeModule.AsyncLocalStorage
    : FallbackAsyncLocalStorage;

export const AsyncResource: any =
  nativeModule && typeof nativeModule.AsyncResource === "function"
    ? nativeModule.AsyncResource
    : FallbackAsyncResource;

const asyncHooks = {
  AsyncLocalStorage,
  AsyncResource,
  default: undefined as any,
};
asyncHooks.default = asyncHooks;

export default asyncHooks;
