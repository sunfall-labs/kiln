// kiln-builtins: node:perf_hooks implementation (Phase 5.2, ADR 0005)

export class PerformanceEntry {
  readonly name: string;
  readonly entryType: string;
  readonly startTime: number;
  readonly duration: number;

  constructor(name: string, entryType: string, startTime: number, duration: number = 0) {
    this.name = String(name);
    this.entryType = String(entryType);
    this.startTime = Number(startTime);
    this.duration = Number(duration);
  }

  toJSON(): any {
    return {
      name: this.name,
      entryType: this.entryType,
      startTime: this.startTime,
      duration: this.duration,
    };
  }
}

export class PerformanceMark extends PerformanceEntry {
  readonly detail: any;

  constructor(name: string, options?: { detail?: any; startTime?: number }) {
    const startTime = options && options.startTime !== undefined ? Number(options.startTime) : performance.now();
    super(name, "mark", startTime, 0);
    this.detail = options ? options.detail : null;
  }
}

export class PerformanceMeasure extends PerformanceEntry {
  readonly detail: any;

  constructor(name: string, options?: { detail?: any; startTime?: number; duration?: number }) {
    const startTime = options && options.startTime !== undefined ? Number(options.startTime) : 0;
    const duration = options && options.duration !== undefined ? Number(options.duration) : 0;
    super(name, "measure", startTime, duration);
    this.detail = options ? options.detail : null;
  }
}

const entriesList: PerformanceEntry[] = [];

export class PerformanceObserverEntryList {
  private _entries: PerformanceEntry[];

  constructor(entries: PerformanceEntry[]) {
    this._entries = entries;
  }

  getEntries(): PerformanceEntry[] {
    return this._entries.slice();
  }

  getEntriesByName(name: string, type?: string): PerformanceEntry[] {
    return this._entries.filter((e) => e.name === name && (!type || e.entryType === type));
  }

  getEntriesByType(type: string): PerformanceEntry[] {
    return this._entries.filter((e) => e.entryType === type);
  }
}

export class PerformanceObserver {
  private _callback: (list: PerformanceObserverEntryList, observer: PerformanceObserver) => void;

  constructor(callback: (list: PerformanceObserverEntryList, observer: PerformanceObserver) => void) {
    if (typeof callback !== "function") {
      throw new TypeError("The \"callback\" argument must be of type function");
    }
    this._callback = callback;
  }

  observe(_options?: { entryTypes?: string[]; type?: string; buffered?: boolean }): void {}

  disconnect(): void {}

  takeRecords(): PerformanceEntry[] {
    return [];
  }
}

export const performance = (globalThis as any).performance;

export const Performance = typeof (globalThis as any).Performance === "function"
  ? (globalThis as any).Performance
  : class Performance {};

export const constants = {
  NODE_PERFORMANCE_GC_MAJOR: 4,
  NODE_PERFORMANCE_GC_MINOR: 1,
  NODE_PERFORMANCE_GC_MINOR_MARK_SWEEP: 2,
  NODE_PERFORMANCE_GC_INCREMENTAL: 8,
  NODE_PERFORMANCE_GC_WEAKCB: 16,
  NODE_PERFORMANCE_GC_FLAGS_NO: 0,
  NODE_PERFORMANCE_GC_FLAGS_CONSTRUCT_RETAINED: 2,
  NODE_PERFORMANCE_GC_FLAGS_FORCED: 4,
  NODE_PERFORMANCE_GC_FLAGS_SYNCHRONOUS_PHANTOM_PROCESSING: 8,
  NODE_PERFORMANCE_GC_FLAGS_ALL_AVAILABLE_GARBAGE: 16,
  NODE_PERFORMANCE_GC_FLAGS_ALL_EXTERNAL_MEMORY: 32,
  NODE_PERFORMANCE_GC_FLAGS_SCHEDULE_IDLE: 64,
};

export function createHistogram(_options?: any) {
  return {
    record(_val: number | bigint) {},
    min: 0,
    max: 0,
    mean: 0,
    stddev: 0,
    percentiles: new Map<number, number>(),
    percentile(_p: number) { return 0; },
    reset() {},
    exceeds: 0,
  };
}

export function monitorEventLoopDelay(_options?: { resolution?: number }) {
  return {
    enable() {},
    disable() {},
    reset() {},
    min: 0,
    max: 0,
    mean: 0,
    stddev: 0,
    percentiles: new Map<number, number>(),
    percentile(_p: number) { return 0; },
    exceeds: 0,
  };
}

export function eventLoopUtilization(_utilization1?: any, _utilization2?: any) {
  return { idle: 0, active: 0, utilization: 0 };
}

export function timerify<T extends (...args: any[]) => any>(fn: T): T {
  return fn;
}

const perfHooksModule = {
  Performance,
  PerformanceEntry,
  PerformanceMark,
  PerformanceMeasure,
  PerformanceObserver,
  PerformanceObserverEntryList,
  constants,
  createHistogram,
  eventLoopUtilization,
  monitorEventLoopDelay,
  performance,
  timerify,
  default: undefined as any,
};
perfHooksModule.default = perfHooksModule;

export default perfHooksModule;
