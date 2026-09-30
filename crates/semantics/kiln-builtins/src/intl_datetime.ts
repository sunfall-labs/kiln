// Native public facade for the shared ICU4X DateTimeFormat engine. The private call returns only
// scalar text/JSON; constructor behavior, methods, and parts objects remain ordinary JS values.
(() => {
  const intl = (globalThis as any).Intl;
  if (intl === undefined || intl === null) return;
  const localeTools = (globalThis as any).__kilnLocaleTools;
  const host = (globalThis as any).__kilnIntlDateTimeFormat;
  delete (globalThis as any).__kilnIntlDateTimeFormat;
  if (typeof host !== "function") return;

  const optionNames = [
    "localeMatcher", "calendar", "numberingSystem", "hour12", "hourCycle", "timeZone", "weekday", "era",
    "year", "month", "day", "dayPeriod", "hour", "minute", "second",
    "fractionalSecondDigits", "timeZoneName", "dateStyle", "timeStyle",
  ];
  const formatters = new WeakMap<object, (value: any) => string>();
  const specifications = new WeakMap<object, string>();

  function DateTimeFormat(this: any, locales?: any, options?: any): any {
    if (!new.target) return new (DateTimeFormat as any)(locales, options);
    const requestedLocales = localeTools.localesList(locales);
    const localeList = JSON.parse(host(JSON.stringify({ locales: requestedLocales, options: {} }), 0, 6));
    if (options === null) throw new TypeError("Intl.DateTimeFormat options must be an object or undefined");
    const source = options === undefined ? {} : Object(options);
    const normalized: any = {};
    for (const key of optionNames) {
      const value = source[key];
      if (value === undefined) continue;
      if (key === "hour12") {
        normalized[key] = !!value;
      } else if (key === "fractionalSecondDigits") {
        const number = +value;
        if (number !== Math.floor(number) || number < 1 || number > 3) {
          throw new RangeError("fractionalSecondDigits must be an integer between 1 and 3");
        }
        normalized[key] = number;
      } else {
        const string = `${value}`;
        if (key === "localeMatcher" && string !== "lookup" && string !== "best fit") {
          throw new RangeError("localeMatcher must be \"lookup\" or \"best fit\"");
        }
        normalized[key] = string;
      }
    }
    const spec = JSON.stringify({ locales: localeList, options: normalized });
    if (spec === undefined) throw new TypeError("Invalid Intl.DateTimeFormat options");
    const invoke = (value: any, operation: number): string => {
      const epoch = value === undefined ? Date.now() : +value;
      return String(host(spec, epoch, operation));
    };
    // Initialize and validate at construction, as ECMA-402 requires. Later calls reuse the
    // provider's bounded per-thread formatter cache.
    host(spec, 0, 2);
    const formatter = function format(value?: any): string { return invoke(value, 0); };
    Object.defineProperty(formatter, "length", { value: 0, configurable: true });
    formatters.set(this, formatter);
    specifications.set(this, spec);
    return this;
  }

  Object.defineProperty(DateTimeFormat, "name", { value: "DateTimeFormat", configurable: true });
  Object.defineProperty(DateTimeFormat, "length", { value: 0, configurable: true });
  Object.defineProperty(DateTimeFormat, "prototype", { writable: false });
  Object.defineProperty(DateTimeFormat.prototype, "format", { configurable: true, get() {
    const formatter = formatters.get(this);
    if (formatter === undefined) throw new TypeError("Intl.DateTimeFormat format getter called on an incompatible receiver");
    return formatter;
  } });
  DateTimeFormat.prototype.formatToParts = function (this: any, value?: any): any[] {
    const spec = specifications.get(this);
    if (spec === undefined) throw new TypeError("Intl.DateTimeFormat method called on an incompatible receiver");
    return JSON.parse(host(spec, value === undefined ? Date.now() : +value, 1));
  };
  DateTimeFormat.prototype.resolvedOptions = function (this: any): any {
    const spec = specifications.get(this);
    if (spec === undefined) throw new TypeError("Intl.DateTimeFormat method called on an incompatible receiver");
    return JSON.parse(host(spec, 0, 2));
  };
  Object.defineProperty(DateTimeFormat.prototype, "constructor", { value: DateTimeFormat, writable: true, configurable: true });
  Object.defineProperty(DateTimeFormat.prototype, Symbol.toStringTag, { value: "Intl.DateTimeFormat", configurable: true });
  Object.defineProperty(DateTimeFormat, "supportedLocalesOf", { value(locales: any, options?: any): string[] {
    const list = localeTools.localesList(locales);
    if (options === null) throw new TypeError("Intl.DateTimeFormat options must be an object or undefined");
    if (options !== undefined) {
      const matcher = (options as any).localeMatcher;
      const matcherValue = matcher === undefined ? undefined : `${matcher}`;
      if (matcherValue !== undefined && matcherValue !== "lookup" && matcherValue !== "best fit") {
        throw new RangeError("localeMatcher must be \"lookup\" or \"best fit\"");
      }
    }
    const spec = JSON.stringify({ locales: list, options: {} });
    return JSON.parse(host(spec, 0, 5));
  }});
  intl.DateTimeFormat = DateTimeFormat;
})();
