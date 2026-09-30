// Native public facade for the shared ICU4X DateTimeFormat engine. The private call returns only
// scalar text/JSON; constructor behavior, methods, and parts objects remain ordinary JS values.
(() => {
  const intl = (globalThis as any).Intl;
  if (intl === undefined || intl === null) return;
  const host = (globalThis as any).__kilnIntlDateTimeFormat;
  delete (globalThis as any).__kilnIntlDateTimeFormat;
  if (typeof host !== "function") return;

  const optionNames = [
    "calendar", "numberingSystem", "hour12", "hourCycle", "timeZone", "weekday", "era",
    "year", "month", "day", "dayPeriod", "hour", "minute", "second",
    "fractionalSecondDigits", "timeZoneName", "dateStyle", "timeStyle",
  ];
  const formatters = new WeakMap<object, (value: any) => string>();
  const specifications = new WeakMap<object, string>();

  function DateTimeFormat(this: any, locales?: any, options?: any): any {
    if (!new.target) throw new TypeError("Intl.DateTimeFormat must be called with new");
    const localeList = locales === undefined ? [] : typeof locales === "string" ? [locales] : Array.from(locales, (tag: any) => {
      if (typeof tag !== "string" && (typeof tag !== "object" || tag === null)) {
        throw new TypeError("Intl.DateTimeFormat locale must be a string or object");
      }
      return String(tag);
    });
    const source = options == null ? {} : Object(options);
    const normalized: any = {};
    for (const key of optionNames) {
      const value = source[key];
      if (value !== undefined) normalized[key] = value;
    }
    const spec = JSON.stringify({ locales: localeList, options: normalized });
    if (spec === undefined) throw new TypeError("Invalid Intl.DateTimeFormat options");
    const invoke = (value: any, operation: number): string => {
      const epoch = value === undefined ? Date.now() : Number(value);
      return String(host(spec, epoch, operation));
    };
    formatters.set(this, (value: any) => invoke(value, 0));
    specifications.set(this, spec);
    Object.defineProperty(this, "format", { configurable: true, get() {
      const formatter = formatters.get(this);
      if (formatter === undefined) throw new TypeError("Intl.DateTimeFormat format getter called on an incompatible receiver");
      return formatter;
    } });
    return this;
  }

  DateTimeFormat.prototype.formatToParts = function (this: any, value?: any): any[] {
    const spec = specifications.get(this);
    if (spec === undefined) throw new TypeError("Intl.DateTimeFormat method called on an incompatible receiver");
    return JSON.parse(host(spec, value === undefined ? Date.now() : Number(value), 1));
  };
  DateTimeFormat.prototype.resolvedOptions = function (this: any): any {
    const spec = specifications.get(this);
    if (spec === undefined) throw new TypeError("Intl.DateTimeFormat method called on an incompatible receiver");
    return JSON.parse(host(spec, 0, 2));
  };
  Object.defineProperty(DateTimeFormat.prototype, "constructor", { value: DateTimeFormat, writable: true, configurable: true });
  Object.defineProperty(DateTimeFormat, "supportedLocalesOf", { value(locales: any): string[] {
    if (locales === undefined) return [];
    const list = typeof locales === "string" ? [locales] : Array.from(locales, (tag: any) => String(tag));
    // Locale availability follows the same resolver as construction; unsupported but well-formed
    // requests fall back, so this list contains the canonical requested tags.
    return list.map((tag: string) => tag);
  }});
  intl.DateTimeFormat = DateTimeFormat;
})();
