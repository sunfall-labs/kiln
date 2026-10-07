// kiln-builtins: `Date.prototype` (ADR 0005, ADR 0063 rule 4, Phase 5.1).
//
// Shared by both interpreter and native execution. The underlying engines only supply
// the heap storage of the instant in milliseconds (`getTime` and `setTime`).
// All calendar arithmetic, field rollover, formatters, and prototype methods are
// defined here in TypeScript to guarantee zero semantic drift.

(function installDatePrototype() {
  // Date's abstract arithmetic must not dispatch through properties that user code can replace
  // on the global Math/Number objects after bootstrap. Setter arguments use unary plus below,
  // which performs the specification's ToNumber operation (including rejecting BigInt).
  const numberIsFinite = Number.isFinite;
  const numberIsNaN = Number.isNaN;
  const mathAbs = Math.abs;
  const mathFloor = Math.floor;
  const mathTrunc = Math.trunc;
  const rawGetTime: (this: Date) => number = Date.prototype.getTime;
  const rawSetTime: (this: Date, time: number) => number = (new Date(0) as any).setTime;
  const rawParse: (dateString: any) => number = Date.parse;
  if (typeof rawGetTime !== "function" || typeof rawSetTime !== "function" ||
      typeof rawParse !== "function") return;
  const parse = { parse(dateString: any): number { return rawParse(dateString); } }.parse;

  function floorMod(a: number, b: number): number {
    const m = a % b;
    return m < 0 ? m + b : m + 0;
  }

  // Howard Hinnant's proleptic Gregorian calendar algorithm
  function civilFromDays(days: number): [number, number, number] {
    const shifted = days + 719468;
    // `Math.floor` already rounds toward negative infinity; the `- 146096` of Hinnant's C
    // original corrects a *truncating* division, and applied here it moved every exact era
    // boundary back an era (`new Date(-8.64e15)` printed a day late).
    const era = mathFloor(shifted / 146097);
    const dayOfEra = shifted - era * 146097;
    const yearOfEra = mathFloor(
      (dayOfEra - mathFloor(dayOfEra / 1460) + mathFloor(dayOfEra / 36524) - mathFloor(dayOfEra / 146096)) / 365
    );
    const computedYear = yearOfEra + era * 400;
    const dayOfYear = dayOfEra - (365 * yearOfEra + mathFloor(yearOfEra / 4) - mathFloor(yearOfEra / 100));
    const shiftedMonth = mathFloor((5 * dayOfYear + 2) / 153);
    const computedDay = dayOfYear - mathFloor((153 * shiftedMonth + 2) / 5) + 1;
    const computedMonth = shiftedMonth < 10 ? shiftedMonth + 3 : shiftedMonth - 9;
    const year = computedYear + (computedMonth <= 2 ? 1 : 0);
    return [year, computedMonth, computedDay];
  }

  function daysFromCivil(year: number, month: number, day: number): number {
    const y = year - (month <= 2 ? 1 : 0);
    const era = mathFloor(y / 400);
    const yearOfEra = y - era * 400;
    const shiftedMonth = month > 2 ? month - 3 : month + 9;
    const dayOfYear = mathFloor((153 * shiftedMonth + 2) / 5) + day - 1;
    const dayOfEra = yearOfEra * 365 + mathFloor(yearOfEra / 4) - mathFloor(yearOfEra / 100) + dayOfYear;
    return era * 146097 + dayOfEra - 719468;
  }

  function timeClip(time: number): number {
    if (!numberIsFinite(time) || mathAbs(time) > 8.64e15) {
      return NaN;
    }
    return mathTrunc(time) + 0;
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
    if (!numberIsFinite(time)) return null;
    const millis = mathTrunc(time);
    const days = mathFloor(millis / 86400000);
    const within = floorMod(millis, 86400000);
    const [year, month1, date] = civilFromDays(days);
    const day = floorMod(days + 4, 7);
    const hour = mathFloor(within / 3600000);
    const minute = mathFloor(within / 60000) % 60;
    const second = mathFloor(within / 1000) % 60;
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
      !numberIsFinite(year) ||
      !numberIsFinite(months) ||
      !numberIsFinite(day) ||
      !numberIsFinite(hour) ||
      !numberIsFinite(minute) ||
      !numberIsFinite(second) ||
      !numberIsFinite(millisecond)
    ) {
      return NaN;
    }
    const y = mathTrunc(year);
    const m = mathTrunc(months);
    const d = mathTrunc(day);
    const h = mathTrunc(hour);
    const min = mathTrunc(minute);
    const s = mathTrunc(second);
    const ms = mathTrunc(millisecond);

    const normYear = y + mathFloor(m / 12);
    const normMonth = floorMod(m, 12);
    if (mathAbs(normYear) > 400000) return NaN;

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
    return numberIsFinite(t) ? civilFromDays(mathFloor(mathTrunc(t) / 86400000))[0] : NaN;
  } }.getUTCFullYear;

  const getUTCMonth = { getUTCMonth(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? civilFromDays(mathFloor(mathTrunc(t) / 86400000))[1] - 1 : NaN;
  } }.getUTCMonth;

  const getUTCDate = { getUTCDate(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? civilFromDays(mathFloor(mathTrunc(t) / 86400000))[2] : NaN;
  } }.getUTCDate;

  const getUTCDay = { getUTCDay(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? floorMod(mathFloor(mathTrunc(t) / 86400000) + 4, 7) : NaN;
  } }.getUTCDay;

  const getUTCHours = { getUTCHours(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? mathFloor(floorMod(mathTrunc(t), 86400000) / 3600000) : NaN;
  } }.getUTCHours;

  const getUTCMinutes = { getUTCMinutes(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? mathFloor(floorMod(mathTrunc(t), 86400000) / 60000) % 60 : NaN;
  } }.getUTCMinutes;

  const getUTCSeconds = { getUTCSeconds(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? mathFloor(floorMod(mathTrunc(t), 86400000) / 1000) % 60 : NaN;
  } }.getUTCSeconds;

  const getUTCMilliseconds = { getUTCMilliseconds(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? floorMod(mathTrunc(t), 86400000) % 1000 : NaN;
  } }.getUTCMilliseconds;

  const getTimezoneOffset = { getTimezoneOffset(this: Date): number {
    // The realm's zone is UTC (ADR 0015), so the offset is 0 -- of a valid time. An invalid
    // date has no offset at all.
    return numberIsNaN(rawGetTime.call(this)) ? NaN : 0;
  } }.getTimezoneOffset;

  const getYear = { getYear(this: Date): number {
    const t = rawGetTime.call(this);
    return numberIsFinite(t) ? civilFromDays(mathFloor(mathTrunc(t) / 86400000))[0] - 1900 : NaN;
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
    const t = +time;
    const clipped = timeClip(t);
    return rawSetTime.call(this, clipped);
  } }.setTime;

  // Every setter converts each argument that is *present* -- counted by `arguments.length`, so
  // an explicit `undefined` is present and converts to NaN -- in order, and only then consults
  // the time value; a setter on an invalid date still runs every `valueOf` it was handed.
  const setUTCFullYear = { setUTCFullYear(this: Date, year: any, month?: any, date?: any): number {
    const t = rawGetTime.call(this);
    const y = +year;
    const base = numberIsNaN(t) ? 0 : t;
    const p = dateParts(base)!;
    const m = arguments.length > 1 ? +month : p.month;
    const d = arguments.length > 2 ? +date : p.date;
    const newTime = partsToInstant(y, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCFullYear;

  const setUTCMonth = { setUTCMonth(this: Date, month: any, date?: any): number {
    const t = rawGetTime.call(this);
    const m = +month;
    const dt = arguments.length > 1 ? +date : undefined;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const d = dt !== undefined ? dt : p.date;
    const newTime = partsToInstant(p.year, m, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCMonth;

  const setUTCDate = { setUTCDate(this: Date, date: any): number {
    const t = rawGetTime.call(this);
    const d = +date;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, d, p.hour, p.minute, p.second, p.millisecond);
    return rawSetTime.call(this, newTime);
  } }.setUTCDate;

  const setUTCHours = { setUTCHours(this: Date, hours: any, min?: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const h = +hours;
    const mi = arguments.length > 1 ? +min : undefined;
    const se = arguments.length > 2 ? +sec : undefined;
    const mil = arguments.length > 3 ? +ms : undefined;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMin = mi !== undefined ? mi : p.minute;
    const newSec = se !== undefined ? se : p.second;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, h, newMin, newSec, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCHours;

  const setUTCMinutes = { setUTCMinutes(this: Date, min: any, sec?: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const m = +min;
    const se = arguments.length > 1 ? +sec : undefined;
    const mil = arguments.length > 2 ? +ms : undefined;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newSec = se !== undefined ? se : p.second;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, m, newSec, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCMinutes;

  const setUTCSeconds = { setUTCSeconds(this: Date, sec: any, ms?: any): number {
    const t = rawGetTime.call(this);
    const s = +sec;
    const mil = arguments.length > 1 ? +ms : undefined;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newMs = mil !== undefined ? mil : p.millisecond;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, s, newMs);
    return rawSetTime.call(this, newTime);
  } }.setUTCSeconds;

  const setUTCMilliseconds = { setUTCMilliseconds(this: Date, ms: any): number {
    const t = rawGetTime.call(this);
    const m = +ms;
    if (numberIsNaN(t)) return NaN;
    const p = dateParts(t)!;
    const newTime = partsToInstant(p.year, p.month, p.date, p.hour, p.minute, p.second, m);
    return rawSetTime.call(this, newTime);
  } }.setUTCMilliseconds;

  const setYear = { setYear(this: Date, year: any): number {
    const t = rawGetTime.call(this);
    let y = +year;
    if (numberIsNaN(y)) {
      rawSetTime.call(this, NaN);
      return NaN;
    }
    if (y >= 0 && y <= 99) {
      y = mathTrunc(y) + 1900;
    }
    const base = numberIsNaN(t) ? 0 : t;
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
    if (typeof tv === "number" && !numberIsFinite(tv)) return null;
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
    if (numberIsNaN(t)) return "Invalid Date";
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
  // One descriptor for every member, not a literal per call: `defineProperty` reads its fields and
  // keeps nothing, and a literal per member was the largest share of the garbage every realm's
  // bootstrap made (realms issue 08).
  const methodDescriptor: PropertyDescriptor = { value: undefined, writable: true, enumerable: false, configurable: true };
  const defineMethod = (name: string | symbol, fn: any) => {
    methodDescriptor.value = fn;
    Object.defineProperty(Date.prototype, name, methodDescriptor);
    methodDescriptor.value = undefined;
  };

  // In the order node's `Object.getOwnPropertyNames(Date.prototype)` reports them: key order is
  // observable, and a program that lists the prototype should see the same list in every engine.
  defineMethod("toString", toString);
  defineMethod("toDateString", toDateString);
  defineMethod("toTimeString", toTimeString);
  defineMethod("toISOString", toISOString);
  defineMethod("toUTCString", toUTCString);
  defineMethod("toGMTString", toUTCString);
  defineMethod("getDate", getDate);
  defineMethod("setDate", setDate);
  defineMethod("getDay", getDay);
  defineMethod("getFullYear", getFullYear);
  defineMethod("setFullYear", setFullYear);
  defineMethod("getHours", getHours);
  defineMethod("setHours", setHours);
  defineMethod("getMilliseconds", getMilliseconds);
  defineMethod("setMilliseconds", setMilliseconds);
  defineMethod("getMinutes", getMinutes);
  defineMethod("setMinutes", setMinutes);
  defineMethod("getMonth", getMonth);
  defineMethod("setMonth", setMonth);
  defineMethod("getSeconds", getSeconds);
  defineMethod("setSeconds", setSeconds);
  defineMethod("getTime", getTime);
  defineMethod("setTime", setTime);
  defineMethod("getTimezoneOffset", getTimezoneOffset);
  defineMethod("getUTCDate", getUTCDate);
  defineMethod("setUTCDate", setUTCDate);
  defineMethod("getUTCDay", getUTCDay);
  defineMethod("getUTCFullYear", getUTCFullYear);
  defineMethod("setUTCFullYear", setUTCFullYear);
  defineMethod("getUTCHours", getUTCHours);
  defineMethod("setUTCHours", setUTCHours);
  defineMethod("getUTCMilliseconds", getUTCMilliseconds);
  defineMethod("setUTCMilliseconds", setUTCMilliseconds);
  defineMethod("getUTCMinutes", getUTCMinutes);
  defineMethod("setUTCMinutes", setUTCMinutes);
  defineMethod("getUTCMonth", getUTCMonth);
  defineMethod("setUTCMonth", setUTCMonth);
  defineMethod("getUTCSeconds", getUTCSeconds);
  defineMethod("setUTCSeconds", setUTCSeconds);
  defineMethod("valueOf", valueOf);
  defineMethod("getYear", getYear);
  defineMethod("setYear", setYear);
  defineMethod("toJSON", toJSON);
  defineMethod("toLocaleString", toLocaleString);
  defineMethod("toLocaleDateString", toLocaleDateString);
  defineMethod("toLocaleTimeString", toLocaleTimeString);
  Object.defineProperty(Date, "parse", {
    value: parse,
    writable: true,
    enumerable: false,
    configurable: true,
  });

  // The one method the specification makes read-only: `{ writable: false, configurable: true }`.
  Object.defineProperty(Date.prototype, Symbol.toPrimitive, {
    value: toPrimitive,
    writable: false,
    enumerable: false,
    configurable: true,
  });
})();
