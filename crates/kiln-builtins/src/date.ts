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
    const era = Math.floor((shifted >= 0 ? shifted : shifted - 146096) / 146097);
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
    const era = Math.floor((y >= 0 ? y : y - 399) / 400);
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
  function getTime(this: Date): number {
    return rawGetTime.call(this);
  }

  function valueOf(this: Date): number {
    return rawGetTime.call(this);
  }

  function getUTCFullYear(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.year : NaN;
  }

  function getUTCMonth(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.month : NaN;
  }

  function getUTCDate(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.date : NaN;
  }

  function getUTCDay(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.day : NaN;
  }

  function getUTCHours(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.hour : NaN;
  }

  function getUTCMinutes(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.minute : NaN;
  }

  function getUTCSeconds(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.second : NaN;
  }

  function getUTCMilliseconds(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.millisecond : NaN;
  }

  function getTimezoneOffset(this: Date): number {
    rawGetTime.call(this);
    return 0;
  }

  function getYear(this: Date): number {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    return p ? p.year - 1900 : NaN;
  }

  function getFullYear(this: Date): number { return getUTCFullYear.call(this); }
  function getMonth(this: Date): number { return getUTCMonth.call(this); }
  function getDate(this: Date): number { return getUTCDate.call(this); }
  function getDay(this: Date): number { return getUTCDay.call(this); }
  function getHours(this: Date): number { return getUTCHours.call(this); }
  function getMinutes(this: Date): number { return getUTCMinutes.call(this); }
  function getSeconds(this: Date): number { return getUTCSeconds.call(this); }
  function getMilliseconds(this: Date): number { return getUTCMilliseconds.call(this); }

  // --- Setters ---
  function setTime(this: Date, time: any): number {
    rawGetTime.call(this);
    const t = Number(time);
    const clipped = timeClip(t);
    return rawSetTime.call(this, clipped);
  }

  function setUTCFullYear(this: Date, year: any, month?: any, date?: any): number {
    const t = rawGetTime.call(this);
    const y = Number(year);
    const base = Number.isNaN(t) ? 0 : t;
    const p = dateParts(base)!;
    const m = month !== undefined ? Number(month) : p.month;
    const d = date !== undefined ? Number(date) : p.date;
    const newTime = partsToInstant(y, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  }

  function setUTCMonth(this: Date, month: any, date?: any): number {
    const t = rawGetTime.call(this);
    const m = Number(month);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const d = date !== undefined ? Number(date) : p.date;
    const newTime = partsToInstant(p.year, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  }

  function setUTCDate(this: Date, date: any): number {
    const t = rawGetTime.call(this);
    const d = Number(date);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  }

  function setUTCHours(this: Date, hours: any, min?: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const h = Number(hours);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMin = min !== undefined ? Number(min) : p.minute;
    const newSec = sec !== undefined ? Number(sec) : p.second;
    const newMs = ms !== undefined ? Number(ms) : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, h, newMin, newSec, newMs);
    return rawSetTime.call(this, newTime);
  }

  function setUTCMinutes(this: Date, min: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const m = Number(min);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newSec = sec !== undefined ? Number(sec) : p.second;
    const newMs = ms !== undefined ? Number(ms) : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, m, newSec, newMs);
    return rawSetTime.call(this, newTime);
  }

  function setUTCSeconds(this: Date, sec: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const s = Number(sec);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMs = ms !== undefined ? Number(ms) : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, s, newMs);
    return rawSetTime.call(this, newTime);
  }

  function setUTCMilliseconds(this: Date, ms: any): number {
    const t = rawGetTime.call(this);
    const m = Number(ms);
    if (Number.isNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, p.second, m);
    return rawSetTime.call(this, newTime);
  }

  function setYear(this: Date, year: any): number {
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
  }

  function setFullYear(this: Date, year: any, month?: any, date?: any): number {
    return setUTCFullYear.apply(this, arguments as any);
  }
  function setMonth(this: Date, month: any, date?: any): number {
    return setUTCMonth.apply(this, arguments as any);
  }
  function setDate(this: Date, date: any): number {
    return setUTCDate.apply(this, arguments as any);
  }
  function setHours(this: Date, hours: any, min?: any, sec?: any, ms?: any): number {
    return setUTCHours.apply(this, arguments as any);
  }
  function setMinutes(this: Date, min: any, sec?: any, ms?: any): number {
    return setUTCMinutes.apply(this, arguments as any);
  }
  function setSeconds(this: Date, sec: any, ms?: any): number {
    return setUTCSeconds.apply(this, arguments as any);
  }
  function setMilliseconds(this: Date, ms: any): number {
    return setUTCMilliseconds.apply(this, arguments as any);
  }

  // --- Formatters ---
  const DAYS = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

  const two = (n: number) => String(n).padStart(2, "0");
  const three = (n: number) => String(n).padStart(3, "0");
  const yearStr4 = (y: number) => (y < 0 ? "-" + String(-y).padStart(4, "0") : String(y).padStart(4, "0"));

  function toDateString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]} ${MONTHS[p.month]} ${two(p.date)} ${yearStr4(p.year)}`;
  }

  function toTimeString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT+0000 (Coordinated Universal Time)`;
  }

  function toString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]} ${MONTHS[p.month]} ${two(p.date)} ${yearStr4(p.year)} ${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT+0000 (Coordinated Universal Time)`;
  }

  function toUTCString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) return "Invalid Date";
    return `${DAYS[p.day]}, ${two(p.date)} ${MONTHS[p.month]} ${yearStr4(p.year)} ${two(p.hour)}:${two(p.minute)}:${two(p.second)} GMT`;
  }

  function toISOString(this: Date): string {
    const t = rawGetTime.call(this);
    const p = dateParts(t);
    if (!p) throw new RangeError("Invalid time value");
    const y = (p.year >= 0 && p.year <= 9999)
      ? String(p.year).padStart(4, "0")
      : (p.year < 0 ? "-" + String(-p.year).padStart(6, "0") : "+" + String(p.year).padStart(6, "0"));
    return `${y}-${two(p.month + 1)}-${two(p.date)}T${two(p.hour)}:${two(p.minute)}:${two(p.second)}.${three(p.millisecond)}Z`;
  }

  function toJSON(this: any, key?: any): any {
    const tv = Number(this.valueOf());
    if (!Number.isFinite(tv)) return null;
    const toISO = this.toISOString;
    if (typeof toISO !== "function") {
      throw new TypeError("toISOString is not a function");
    }
    return toISO.call(this);
  }

  function toLocaleString(this: Date): string {
    if (typeof Intl !== "undefined" && typeof (Intl as any).DateTimeFormat === "function") {
      return new (Intl as any).DateTimeFormat(arguments[0], arguments[1]).format(this);
    }
    return toString.call(this);
  }

  function toLocaleDateString(this: Date): string {
    if (typeof Intl !== "undefined" && typeof (Intl as any).DateTimeFormat === "function") {
      return new (Intl as any).DateTimeFormat(arguments[0], arguments[1]).format(this);
    }
    return toDateString.call(this);
  }

  function toLocaleTimeString(this: Date): string {
    if (typeof Intl !== "undefined" && typeof (Intl as any).DateTimeFormat === "function") {
      return new (Intl as any).DateTimeFormat(arguments[0], arguments[1]).format(this);
    }
    return toTimeString.call(this);
  }

  function toPrimitive(this: any, hint: string): any {
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
  }

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
  defineMethod(Symbol.toPrimitive, toPrimitive);
})();
