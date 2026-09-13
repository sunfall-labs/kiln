// kiln-builtins: node:console implementation (Phase 5.2, ADR 0005)

const globalConsole = (globalThis as any).console;

export const Console = typeof globalConsole?.Console === "function"
  ? globalConsole.Console
  : class Console {
      log(...args: any[]): void {
        globalConsole?.log?.(...args);
      }
      error(...args: any[]): void {
        globalConsole?.error?.(...args);
      }
      warn(...args: any[]): void {
        globalConsole?.warn?.(...args);
      }
      info(...args: any[]): void {
        globalConsole?.info?.(...args);
      }
      debug(...args: any[]): void {
        if (typeof globalConsole?.debug === "function") {
          globalConsole.debug(...args);
        } else {
          globalConsole?.log?.(...args);
        }
      }
    };

export const log = (...args: any[]): void => {
  globalConsole?.log?.(...args);
};

export const error = (...args: any[]): void => {
  globalConsole?.error?.(...args);
};

export const warn = (...args: any[]): void => {
  globalConsole?.warn?.(...args);
};

export const info = (...args: any[]): void => {
  globalConsole?.info?.(...args);
};

export const debug = (...args: any[]): void => {
  if (typeof globalConsole?.debug === "function") {
    globalConsole.debug(...args);
  } else {
    globalConsole?.log?.(...args);
  }
};

export const assert = (value: any, ...message: any[]): void => {
  if (typeof globalConsole?.assert === "function") {
    globalConsole.assert(value, ...message);
  } else if (!value) {
    globalConsole?.error?.("Assertion failed:", ...message);
  }
};

export const clear = (): void => {
  globalConsole?.clear?.();
};

export const count = (label?: string): void => {
  globalConsole?.count?.(label);
};

export const countReset = (label?: string): void => {
  globalConsole?.countReset?.(label);
};

export const dir = (item?: any, options?: any): void => {
  if (typeof globalConsole?.dir === "function") {
    globalConsole.dir(item, options);
  } else {
    globalConsole?.log?.(item);
  }
};

export const dirxml = (...args: any[]): void => {
  if (typeof globalConsole?.dirxml === "function") {
    globalConsole.dirxml(...args);
  } else {
    globalConsole?.log?.(...args);
  }
};

export const group = (...label: any[]): void => {
  globalConsole?.group?.(...label);
};

export const groupCollapsed = (...label: any[]): void => {
  globalConsole?.groupCollapsed?.(...label);
};

export const groupEnd = (): void => {
  globalConsole?.groupEnd?.();
};

export const table = (tabularData?: any, properties?: string[]): void => {
  if (typeof globalConsole?.table === "function") {
    globalConsole.table(tabularData, properties);
  } else {
    globalConsole?.log?.(tabularData);
  }
};

export const time = (label?: string): void => {
  globalConsole?.time?.(label);
};

export const timeEnd = (label?: string): void => {
  globalConsole?.timeEnd?.(label);
};

export const timeLog = (label?: string, ...data: any[]): void => {
  globalConsole?.timeLog?.(label, ...data);
};

export const trace = (...args: any[]): void => {
  if (typeof globalConsole?.trace === "function") {
    globalConsole.trace(...args);
  } else {
    globalConsole?.error?.("Trace:", ...args);
  }
};

export const profile = (label?: string): void => {
  globalConsole?.profile?.(label);
};

export const profileEnd = (label?: string): void => {
  globalConsole?.profileEnd?.(label);
};

export const timeStamp = (label?: string): void => {
  globalConsole?.timeStamp?.(label);
};

const consoleModule = {
  Console,
  assert,
  clear,
  count,
  countReset,
  debug,
  dir,
  dirxml,
  error,
  group,
  groupCollapsed,
  groupEnd,
  info,
  log,
  profile,
  profileEnd,
  table,
  time,
  timeEnd,
  timeLog,
  timeStamp,
  trace,
  warn,
  default: undefined as any,
};
consoleModule.default = consoleModule;

export default consoleModule;
