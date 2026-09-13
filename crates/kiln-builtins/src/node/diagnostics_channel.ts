// kiln-builtins: node:diagnostics_channel implementation (Phase 5.2, ADR 0005)

const channelRegistry: Record<string, Channel> = Object.create(null);

export class Channel {
  readonly name: string;
  private _subscribers: Array<(message: any, name: string) => void> = [];
  private _stores: Array<{ store: any; transform?: (msg: any) => any }> = [];

  constructor(name: string) {
    this.name = String(name);
  }

  get hasSubscribers(): boolean {
    return this._subscribers.length > 0;
  }

  subscribe(subscription: (message: any, name: string) => void): void {
    if (typeof subscription !== "function") {
      throw new TypeError("The \"subscription\" argument must be of type function");
    }
    if (this._subscribers.indexOf(subscription) === -1) {
      this._subscribers.push(subscription);
    }
  }

  unsubscribe(subscription: (message: any, name: string) => void): boolean {
    const idx = this._subscribers.indexOf(subscription);
    if (idx !== -1) {
      this._subscribers.splice(idx, 1);
      return true;
    }
    return false;
  }

  publish(data: any): void {
    const subs = this._subscribers;
    for (let i = 0; i < subs.length; i++) {
      subs[i](data, this.name);
    }
  }

  bindStore(store: any, transform?: (msg: any) => any): void {
    this._stores.push({ store, transform });
  }

  unbindStore(store: any): boolean {
    for (let i = 0; i < this._stores.length; i++) {
      if (this._stores[i].store === store) {
        this._stores.splice(i, 1);
        return true;
      }
    }
    return false;
  }

  runStores(
    context: any,
    fn: (...args: any[]) => any,
    thisArg?: any,
    ...args: any[]
  ): any {
    if (this._stores.length === 0) {
      return fn.apply(thisArg, args);
    }
    let runner = () => fn.apply(thisArg, args);
    for (let i = this._stores.length - 1; i >= 0; i--) {
      const entry = this._stores[i];
      const val = entry.transform ? entry.transform(context) : context;
      const nextRunner = runner;
      runner = () => entry.store.run(val, nextRunner);
    }
    return runner();
  }
}

export class TracingChannel {
  readonly start: Channel;
  readonly end: Channel;
  readonly asyncStart: Channel;
  readonly asyncEnd: Channel;
  readonly error: Channel;

  constructor(nameOrChannels: string | Record<string, Channel>) {
    if (typeof nameOrChannels === "string") {
      this.start = channel(`tracing:${nameOrChannels}:start`);
      this.end = channel(`tracing:${nameOrChannels}:end`);
      this.asyncStart = channel(`tracing:${nameOrChannels}:asyncStart`);
      this.asyncEnd = channel(`tracing:${nameOrChannels}:asyncEnd`);
      this.error = channel(`tracing:${nameOrChannels}:error`);
    } else if (typeof nameOrChannels === "object" && nameOrChannels !== null) {
      this.start = nameOrChannels.start;
      this.end = nameOrChannels.end;
      this.asyncStart = nameOrChannels.asyncStart;
      this.asyncEnd = nameOrChannels.asyncEnd;
      this.error = nameOrChannels.error;
    } else {
      throw new TypeError("The \"nameOrChannels\" argument must be of type string or an object");
    }
  }

  get hasSubscribers(): boolean {
    return (
      this.start.hasSubscribers ||
      this.end.hasSubscribers ||
      this.asyncStart.hasSubscribers ||
      this.asyncEnd.hasSubscribers ||
      this.error.hasSubscribers
    );
  }

  subscribe(subscribers: {
    start?: (message: any, name: string) => void;
    end?: (message: any, name: string) => void;
    asyncStart?: (message: any, name: string) => void;
    asyncEnd?: (message: any, name: string) => void;
    error?: (message: any, name: string) => void;
  }): void {
    if (subscribers.start) this.start.subscribe(subscribers.start);
    if (subscribers.end) this.end.subscribe(subscribers.end);
    if (subscribers.asyncStart) this.asyncStart.subscribe(subscribers.asyncStart);
    if (subscribers.asyncEnd) this.asyncEnd.subscribe(subscribers.asyncEnd);
    if (subscribers.error) this.error.subscribe(subscribers.error);
  }

  unsubscribe(subscribers: {
    start?: (message: any, name: string) => void;
    end?: (message: any, name: string) => void;
    asyncStart?: (message: any, name: string) => void;
    asyncEnd?: (message: any, name: string) => void;
    error?: (message: any, name: string) => void;
  }): void {
    if (subscribers.start) this.start.unsubscribe(subscribers.start);
    if (subscribers.end) this.end.unsubscribe(subscribers.end);
    if (subscribers.asyncStart) this.asyncStart.unsubscribe(subscribers.asyncStart);
    if (subscribers.asyncEnd) this.asyncEnd.unsubscribe(subscribers.asyncEnd);
    if (subscribers.error) this.error.unsubscribe(subscribers.error);
  }

  traceSync(fn: (...args: any[]) => any, context: any = {}, thisArg?: any, ...args: any[]): any {
    this.start.publish(context);
    try {
      const result = fn.apply(thisArg, args);
      context.result = result;
      this.end.publish(context);
      return result;
    } catch (err) {
      context.error = err;
      this.error.publish(context);
      throw err;
    }
  }

  tracePromise(fn: (...args: any[]) => any, context: any = {}, thisArg?: any, ...args: any[]): Promise<any> {
    this.start.publish(context);
    try {
      const promise = Promise.resolve(fn.apply(thisArg, args));
      return promise.then(
        (result) => {
          context.result = result;
          this.end.publish(context);
          return result;
        },
        (err) => {
          context.error = err;
          this.error.publish(context);
          throw err;
        }
      );
    } catch (err) {
      context.error = err;
      this.error.publish(context);
      return Promise.reject(err);
    }
  }
}

export function channel(name: string): Channel {
  const key = String(name);
  let chan = channelRegistry[key];
  if (!chan) {
    chan = new Channel(key);
    channelRegistry[key] = chan;
  }
  return chan;
}

export function hasSubscribers(name: string): boolean {
  const chan = channelRegistry[String(name)];
  return chan ? chan.hasSubscribers : false;
}

export function subscribe(
  name: string,
  subscription: (message: any, name: string) => void
): void {
  channel(name).subscribe(subscription);
}

export function unsubscribe(
  name: string,
  subscription: (message: any, name: string) => void
): boolean {
  return channel(name).unsubscribe(subscription);
}

export function tracingChannel(
  nameOrChannels: string | Record<string, Channel>
): TracingChannel {
  return new TracingChannel(nameOrChannels);
}

const diagnosticsChannelModule = {
  Channel,
  TracingChannel,
  channel,
  hasSubscribers,
  subscribe,
  tracingChannel,
  unsubscribe,
  default: undefined as any,
};
diagnosticsChannelModule.default = diagnosticsChannelModule;

export default diagnosticsChannelModule;
