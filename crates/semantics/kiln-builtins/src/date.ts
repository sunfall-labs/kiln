// kiln-builtins: `Date.prototype` (ADR 0005, ADR 0063 rule 4, Phase 5.1).
//
// Shared by both interpreter and native execution. The underlying engines only supply
// the heap storage of the instant in milliseconds (`getTime` and `setTime`).
// All calendar arithmetic, field rollover, formatters, and prototype methods are
// defined here in TypeScript to guarantee zero semantic drift.

(function installDatePrototype() {
  const rawGetTime: (this: Date) => number = Date.prototype.getTime;
  const rawSetTime: (this: Date, time: number) => number = (new Date(0) as any).setTime;
  if (typeof rawGetTime !== "function" || typeof rawSetTime !== "function") return;

  function floorMod(a: number, b: number): number {
    const m = a % b;
    return m < 0 ? m + b : m;
  }

  // Howard Hinnant's proleptic Gregorian calendar algorithm
  function civilFromDays(days: number): [number, number, number] {
    const shifted = days + 719468;
    // `Math.floor` already rounds toward negative infinity; the `- 146096` of Hinnant's C
    // original corrects a *truncating* division, and applied here it moved every exact era
    // boundary back an era (`new Date(-8.64e15)` printed a day late).
    const era = Math.floor(shifted / 146097);
    const dayOfEra = shifted - era * 146097;
    const yearOfEra = Math.floor(
      (dayOfEra - Math.floor(dayOfEra / 1460) + Math.floor(dayOfEra / 36524) - Math.floor(dayOfEra / 146096)) / 365
    );
    const computedYear = yearOfEra + era * 400;
    const dayOfYear = dayOfEra - (365 * yearOfEra + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100));
    const shiftedMonth = Math.floor((5 * dayOfYear + 2) / 153);
    const computedDay = dayOfYear - Math.floor((153 * shiftedMonth + 2) / 5) + 1;
    const computedMonth = shiftedMonth < 10 ? shiftedMonth + 3 : shiftedMonth - 9;
    const year = computedYear + (computedMonth <= 2 ? 1 : 0);
    return [year, computedMonth, computedDay];
  }

  function daysFromCivil(year: number, month: number, day: number): number {
    const y = year - (month <= 2 ? 1 : 0);
    const era = Math.floor(y / 400);
    const yearOfEra = y - era * 400;
    const shiftedMonth = month > 2 ? month - 3 : month + 9;
    const dayOfYear = Math.floor((153 * shiftedMonth + 2) / 5) + day - 1;
    const dayOfEra = yearOfEra * 365 + Math.floor(yearOfEra / 4) - Math.floor(yearOfEra / 100) + dayOfYear;
    return era * 146097 + dayOfEra - 719468;
  }

  function timeClip(time: number): number {
    if (!Number.isFinite(time) || Math.abs(time) > 8.64e15) {
      return NaN;
    }
    return Math.trunc(time) + 0;
  }

  interface DateParts {
    year: number;
    month: number; // 0..11
    date: number;  // 1..31
    day: number;   // 0..6 (0 = Sun)
    hour: number;  // 0..23
    minute: number;// 0..59
    second: number;// 0..59
    millisecond: number; // 0..999
  }

  function dateParts(time: number): DateParts | null {
    if (!Number.isFinite(time)) return null;
    const millis = Math.trunc(time);
    const days = Math.floor(millis / 86400000);
    const within = floorMod(millis, 86400000);
    const [year, month1, date] = civilFromDays(days);
    const day = floorMod(days + 4, 7);
    const hour = Math.floor(within / 3600000);
    const minute = Math.floor(within / 60000) % 60;
    const second = Math.floor(within / 1000) % 60;
    const millisecond = within % 1000;
    return {
      year,
      month: month1 - 1,
      date,
      day,
      hour,
      minute,
      second,
      millisecond,
    };
  }

  function partsToInstant(
    year: number,
    months: number,
    day: number,
    hour: number,
    minute: number,
    second: number,
    millisecond: number
  ): number {
    if (
      !Number.isFinite(year) ||
      !Number.isFinite(months) ||
      !Number.isFinite(day) ||
      !Number.isFinite(hour) ||
      !Number.isFinite(minute) ||
      !Number.isFinite(second) ||
      !Number.isFinite(millisecond)
    ) {
      return NaN;
    }
    const y = Math.trunc(year);
    const m = Math.trunc(months);
    const d = Math.trunc(day);
    const h = Math.trunc(hour);
    const min = Math.trunc(minute);
    const s = Math.trunc(second);
    const ms = Math.trunc(millisecond);

    const normYear = y + Math.floor(m / 12);
    const normMonth = floorMod(m, 12);
    if (Math.abs(normYear) > 400000) return NaN;

    const days = daysFromCivil(normYear, normMonth + 1, 1) + (d - 1);
    const clock = ((h * 3600000 + min * 60000) + s * 1000) + ms;
    return timeClip(days * 86400000 + clock);
  }

  // --- Getters ---
  const getTime = { getTime(this: Date): number {
    return rawGetTime.call(this);
  } }.getTime;

  const valueOf = { valueOf(this: Date): number {
    return rawGetTime.call(this);
  } }.valueOf;

  const getUTCFullYear = { getUTCFullYear(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.year : NaN;
  } }.getUTCFullYear;

  const getUTCMonth = { getUTCMonth(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.month : NaN;
  } }.getUTCMonth;

  const getUTCDate = { getUTCDate(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.date : NaN;
  } }.getUTCDate;

  const getUTCDay = { getUTCDay(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.day : NaN;
  } }.getUTCDay;

  const getUTCHours = { getUTCHours(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.hour : NaN;
  } }.getUTCHours;

  const getUTCMinutes = { getUTCMinutes(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.minute : NaN;
  } }.getUTCMinutes;

  const getUTCSeconds = { getUTCSeconds(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.second : NaN;
  } }.getUTCSeconds;

  const getUTCMilliseconds = { getUTCMilliseconds(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.millisecond : NaN;
  } }.getUTCMilliseconds;

  const getTimezoneOffset = { getTimezoneOffset(this: Date): number {
    // The realm's zone is UTC (ADR 0015), so the offset is 0 -- of a valid time. An invalid
    // date has no offset at all.
    return Number.isNaN(rawGetTime.call(this)) ? NaN : 0;
  } }.getTimezoneOffset;

  const getYear = { getYear(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.year - 1900 : NaN;
  } }.getYear;

  const getFullYear = { getFullYear(this: Date): number { return getUTCFullYear.call(this); } }.getFullYear;
  const getMonth = { getMonth(this: Date): number { return getUTCMonth.call(this); } }.getMonth;
  const getDate = { getDate(this: Date): number { return getUTCDate.call(this); } }.getDate;
  const getDay = { getDay(this: Date): number { return getUTCDay.call(this); } }.getDay;
  const getHours = { getHours(this: Date): number { return getUTCHours.call(this); } }.getHours;
  const getMinutes = { getMinutes(this: Date): number { return getUTCMinutes.call(this); } }.getMinutes;
  const getSeconds = { getSeconds(this: Date): number { return getUTCSeconds.call(this); } }.getSeconds;
  const getMilliseconds = { getMilliseconds(this: Date): number { return getUTCMilliseconds.call(this); } }.getMilliseconds;

  // --- Setters ---
  const setTime = { setTime(this: Date, time: any): number {
    rawGetTime.call(this);
    const t = Number(time);
    const clipped = timeClip(t);
    return rawSetTime.call(this, clipped);
  } }.setTime;

  // Every setter converts each argument that is *present* -- counted by `arguments.length`, so
  // an explicit `undefined` is present and converts to NaN -- in order, and only then consults
  // the time value; a setter on an invalid date still runs every `valueOf` it was handed.
  const setUTCFullYear = { setUTCFullYear(this: Date, year: any, month?: any, date?: any): number {
    const t = rawGetTime.call(this);
    const y = Number(year);
    const base = Number.isNaN(t) ? 0 : t;
    const p = dateParts(base)!;
    const m = arguments.length > 1 ? Number(month) : p.month;
    const d = arguments.length > 2 ? Number(date) : p.date;
    const newTime = partsToInstant(y, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCFullYear;

  const setUTCMonth = { setUTCMonth(this: Date, month: any, date?: any): number {
    const t = rawGetTime.call(this);
    const m = Number(month);
    const dt = arguments.length > 1 ? Number(date) : undefined;
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const d = dt !== undefined ? dt : p.date;
    const newTime = partsToInstant(p.year, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCMonth;

  const setUTCDate = { setUTCDate(this: Date, date: any): number {
    const t = rawGetTime.call(this);
    const d = Number(date);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCDate;

  const setUTCHours = { setUTCHours(this: Date, hours: any, min?: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const h = Number(hours);
    const mi = arguments.length > 1 ? Number(min) : undefined;
    const se = arguments.length > 2 ? Number(sec) : undefined;
    const mil = arguments.length > 3 ? Number(ms) : undefined;
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMin = mi !== undefined ? mi : p.minute;
    const newSec = se !== undefined ? se : p.second;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, h, newMin, newSec, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCHours;

  const setUTCMinutes = { setUTCMinutes(this: Date, min: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const m = Number(min);
    const se = arguments.length > 1 ? Number(sec) : undefined;
    const mil = arguments.length > 2 ? Number(ms) : undefined;
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newSec = se !== undefined ? se : p.second;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, m, newSec, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCMinutes;

  const setUTCSeconds = { setUTCSeconds(this: Date, sec: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const s = Number(sec);
    const mil = arguments.length > 1 ? Number(ms) : undefined;
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, s, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCSeconds;

  const setUTCMilliseconds = { setUTCMilliseconds(this: Date, ms: any): number {
    const t = rawGetTime.call(this);
    const m = Number(ms);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, p.second, m);
    return rawSetTime.call(this, newTime);
  } }.setUTCMilliseconds;

  const setYear = { setYear(this: Date, year: any): number {
    const t = rawGetTime.call(this);
    let y = Number(year);
    if (Number.isNaN(y)) {
      rawSetTime.call(this, NaN);
      return NaN;
    }
    if (y >= 0 && y <= 99) {
      y = Math.trunc(y) + 1900;
    }
    const base = Number.isNaN(t) ? 0 : t;
    const p = dateParts(base)!;
    const newTime = partsToInstant(y, p.month, p.date, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setYear;

  const setFullYear = { setFullYear(this: Date, year: any, month?: any, date?: any): number {
    return setUTCFullYear.apply(this, arguments as any);
  } }.setFullYear;
  const setMonth = { setMonth(this: Date, month: any, date?: any): number {
    return setUTCMonth.apply(this, arguments as any);
  } }.setMonth;
  const setDate = { setDate(this: Date, date: any): number {
    return setUTCDate.apply(this, arguments as any);
  } }.setDate;
  const setHours = { setHours(this: Date, hours: any, min?: any, sec?: any, ms?: any): number {
    return setUTCHours.apply(this, arguments as any);
  } }.setHours;
  const setMinutes = { setMinutes(this: Date, min: any, sec?: any, ms?: any): number {
    return setUTCMinutes.apply(this, arguments as any);
  } }.setMinutes;
  const setSeconds = { setSeconds(this: Date, sec: any, ms?: any): number {
    return setUTCSeconds.apply(this, arguments as any);
  } }.setSeconds;
  const setMilliseconds = { setMilliseconds(this: Date, ms: any): number {
    return setUTCMilliseconds.apply(this, arguments as any);
  } }.setMilliseconds;

  // --- Formatters ---
  const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  const two = (n: number) => String(n).padStart(2, "0");
  const three = (n: number) => String(n).padStart(3, "0");
  const yearStr4 = (y: number) => (y < 0 ? "-" + String(-y).padStart(4, "0") : String(y).padStart(4, "0"));

  const toDateString = { toDateString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]} ${MONTHS[p.month]} ${two(p.date)} ${yearStr4(p.year)}`;
  } }.toDateString;

  const toTimeString = { toTimeString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT+0000 (Coordinated Universal Time)`;
  } }.toTimeString;

  const toString = { toString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]} ${MONTHS[p.month]} ${two(p.date)} ${yearStr4(p.year)} ${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT+0000 (Coordinated Universal Time)`;
  } }.toString;

  const toUTCString = { toUTCString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]}, ${two(p.date)} ${MONTHS[p.month]} ${yearStr4(p.year)} ${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT`;
  } }.toUTCString;

  const toISOString = { toISOString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) throw new RangeError("Invalid time value");
    const y = (p.year >= 0 && p.year <= 9999)
      ? String(p.year).padStart(4, "0")
      : (p.year < 0 ? "-" + String(-p.year).padStart(6, "0") : "+" + String(p.year).padStart(6, "0"));
    return `${y}-${two(p.month + 1)}-${two(p.date)}T${two(p.hour)}:${two(p.minute)}:${two(p.second)}.${three(p.millisecond)}Z`;
  } }.toISOString;

  // `Date.prototype.toJSON`: generic -- any object with a `toISOString` will do -- so it asks the
  // receiver's own `ToPrimitive` with the number hint rather than its time value.
  const toJSON = { toJSON(this: any, key: any): any {
    if (this === null || this === undefined) {
      throw new TypeError("Date.prototype.toJSON called on null or undefined");
    }
    const O = Object(this);
    const tv = toPrimitiveNumber(O);
    if (typeof tv === "number" && !Number.isFinite(tv)) return null;
    const toISO = O.toISOString;
    if (typeof toISO !== "function") {
      throw new TypeError("toISOString is not a function");
    }
    return toISO.call(O);
  } }.toJSON;

  // `ToPrimitive(input, number)` for an object.
  function toPrimitiveNumber(input: any): any {
    const exotic = input[Symbol.toPrimitive];
    if (exotic !== undefined && exotic !== null) {
      if (typeof exotic !== "function") throw new TypeError("Symbol.toPrimitive is not a function");
      const result = exotic.call(input, "number");
      if (result !== null && (typeof result === "object" || typeof result === "function")) {
        throw new TypeError("Cannot convert object to primitive value");
      }
      return result;
    }
    for (const name of ["valueOf", "toString"]) {
      const method = input[name];
      if (typeof method === "function") {
        const result = method.call(input);
        if (result === null || (typeof result !== "object" && typeof result !== "function")) {
          return result;
        }
      }
    }
    throw new TypeError("Cannot convert object to primitive value");
  }

  // `ToDateTimeOptions` (ECMA-402): a formatter asked for by `toLocale*String` has the fields
  // its method implies unless the caller named some of its own -- all of them for
  // `toLocaleString`, the date for `toLocaleDateString`, the time for `toLocaleTimeString`. The
  // caller's options are the prototype of the object handed on, so nothing is copied.
  function localeOptions(options: any, required: string, defaults: string): any {
    const o = Object.create(options === undefined ? null : Object(options));
    let needDefaults = true;
    if (required === "date" || required === "any") {
      for (const name of ["weekday", "year", "month", "day"]) {
        if (o[name] !== undefined) needDefaults = false;
      }
    }
    if (required === "time" || required === "any") {
      for (const name of ["dayPeriod", "hour", "minute", "second", "fractionalSecondDigits"]) {
        if (o[name] !== undefined) needDefaults = false;
      }
    }
    if (o.dateStyle !== undefined || o.timeStyle !== undefined) needDefaults = false;
    if (required === "date" && o.timeStyle !== undefined) {
      throw new TypeError("toLocaleDateString does not accept timeStyle");
    }
    if (required === "time" && o.dateStyle !== undefined) {
      throw new TypeError("toLocaleTimeString does not accept dateStyle");
    }
    if (needDefaults && (defaults === "date" || defaults === "all")) {
      o.year = "numeric";
      o.month = "numeric";
      o.day = "numeric";
    }
    if (needDefaults && (defaults === "time" || defaults === "all")) {
      o.hour = "numeric";
      o.minute = "numeric";
      o.second = "numeric";
    }
    return o;
  }

  // Each takes the time value first -- a TypeError for anything that is not a Date -- and an
  // invalid date is "Invalid Date" whatever the locale.
  function toLocaleWith(date: Date, locales: any, options: any, required: string, defaults: string, fallback: (this: Date) => string): string {
    const t = rawGetTime.call(date);
    if (Number.isNaN(t)) return "Invalid Date";
    if (typeof Intl !== "undefined" && typeof (Intl as any).DateTimeFormat === "function") {
      return new (Intl as any).DateTimeFormat(locales, localeOptions(options, required, defaults)).format(t);
    }
    return fallback.call(date);
  }

  const toLocaleString = { toLocaleString(this: Date, locales: any = undefined, options: any = undefined): string {
    return toLocaleWith(this, locales, options, "any", "all", toString);
  } }.toLocaleString;

  const toLocaleDateString = { toLocaleDateString(this: Date, locales: any = undefined, options: any = undefined): string {
    return toLocaleWith(this, locales, options, "date", "date", toDateString);
  } }.toLocaleDateString;

  const toLocaleTimeString = { toLocaleTimeString(this: Date, locales: any = undefined, options: any = undefined): string {
    return toLocaleWith(this, locales, options, "time", "time", toTimeString);
  } }.toLocaleTimeString;

  const toPrimitive = { [Symbol.toPrimitive](this: any, hint: string): any {
    if (typeof this !== "object" || this === null) {
      throw new TypeError("Date.prototype[Symbol.toPrimitive] called on a non-object");
    }
    if (hint === "string" || hint === "default") {
      const str = this.toString;
      if (typeof str === "function") {
        const val = str.call(this);
        if (typeof val !== "object" || val === null) return val;
      }
      const valOf = this.valueOf;
      if (typeof valOf === "function") {
        const val = valOf.call(this);
        if (typeof val !== "object" || val === null) return val;
      }
      throw new TypeError("Cannot convert object to primitive value");
    } else if (hint === "number") {
      const valOf = this.valueOf;
      if (typeof valOf === "function") {
        const val = valOf.call(this);
        if (typeof val !== "object" || val === null) return val;
      }
      const str = this.toString;
      if (typeof str === "function") {
        const val = str.call(this);
        if (typeof val !== "object" || val === null) return val;
      }
      throw new TypeError("Cannot convert object to primitive value");
    }
    throw new TypeError("Invalid hint: " + String(hint));
  } }[Symbol.toPrimitive];

  // Built-in methods are not constructors, so each is a method definition (`{ m() {} }.m`)
  // rather than a function declaration, which would have [[Construct]].
  const defineMethod = (name: string | symbol, fn: any) => {
    Object.defineProperty(Date.prototype, name, {
      value: fn,
      writable: true,
      enumerable: false,
      configurable: true,
    });
  };

  defineMethod("getTime", getTime);
  defineMethod("valueOf", valueOf);
  defineMethod("getFullYear", getFullYear);
  defineMethod("getUTCFullYear", getUTCFullYear);
  defineMethod("getMonth", getMonth);
  defineMethod("getUTCMonth", getUTCMonth);
  defineMethod("getDate", getDate);
  defineMethod("getUTCDate", getUTCDate);
  defineMethod("getDay", getDay);
  defineMethod("getUTCDay", getUTCDay);
  defineMethod("getHours", getHours);
  defineMethod("getUTCHours", getUTCHours);
  defineMethod("getMinutes", getMinutes);
  defineMethod("getUTCMinutes", getUTCMinutes);
  defineMethod("getSeconds", getSeconds);
  defineMethod("getUTCSeconds", getUTCSeconds);
  defineMethod("getMilliseconds", getMilliseconds);
  defineMethod("getUTCMilliseconds", getUTCMilliseconds);
  defineMethod("getTimezoneOffset", getTimezoneOffset);
  defineMethod("getYear", getYear);

  defineMethod("setTime", setTime);
  defineMethod("setFullYear", setFullYear);
  defineMethod("setUTCFullYear", setUTCFullYear);
  defineMethod("setMonth", setMonth);
  defineMethod("setUTCMonth", setUTCMonth);
  defineMethod("setDate", setDate);
  defineMethod("setUTCDate", setUTCDate);
  defineMethod("setHours", setHours);
  defineMethod("setUTCHours", setUTCHours);
  defineMethod("setMinutes", setMinutes);
  defineMethod("setUTCMinutes", setUTCMinutes);
  defineMethod("setSeconds", setSeconds);
  defineMethod("setUTCSeconds", setUTCSeconds);
  defineMethod("setMilliseconds", setMilliseconds);
  defineMethod("setUTCMilliseconds", setUTCMilliseconds);
  defineMethod("setYear", setYear);

  defineMethod("toDateString", toDateString);
  defineMethod("toTimeString", toTimeString);
  defineMethod("toString", toString);
  defineMethod("toUTCString", toUTCString);
  defineMethod("toGMTString", toUTCString);
  defineMethod("toISOString", toISOString);
  defineMethod("toJSON", toJSON);
  defineMethod("toLocaleString", toLocaleString);
  defineMethod("toLocaleDateString", toLocaleDateString);
  defineMethod("toLocaleTimeString", toLocaleTimeString);
  // The one method the specification makes read-only: `{ writable: false, configurable: true }`.
  Object.defineProperty(Date.prototype, Symbol.toPrimitive, {
    value: toPrimitive,
    writable: false,
    enumerable: false,
    configurable: true,
  });
})();
