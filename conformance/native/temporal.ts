// `Temporal` under both engines, through the one linked provider (ADR 0025, ADR 0029 §4).
//
// The engine is `kiln-temporal` over `temporal_rs` in both cases -- the interpreter links the
// crate and the native artifact links the same crate through the provider archive -- so a
// disagreement here is the object model around it, not a calendar.
//
// Only the operations the native backend actually carries are exercised. What is still refused by
// name under `--native` -- `epochNanoseconds`, which only a BigInt can hold, and the static
// `fromEpochMilliseconds` pair -- stays out, because a probe that printed several lines and then
// refused would read as a divergence rather than as the refusal it is.

const date = Temporal.PlainDate.from({ year: 2026, month: 8, day: 30 });
console.log(date.toString());
console.log(date.year, date.month, date.day);
console.log(date.dayOfWeek, date.dayOfYear, date.daysInMonth, date.daysInYear);
console.log(date.monthCode, date.calendarId, date.inLeapYear);

// A leap day, which is where a calendar that was reimplemented rather than linked would drift.
const leap = Temporal.PlainDate.from({ year: 2024, month: 2, day: 29 });
console.log(leap.toString(), leap.inLeapYear, leap.daysInYear);

// ISO text in, the same value out.
console.log(Temporal.PlainDate.from("2019-11-18").toString());
console.log(Temporal.PlainTime.from("12:34:56.789").toString());
console.log(Temporal.PlainDateTime.from("2020-01-01T05:06:07").toString());
console.log(Temporal.PlainYearMonth.from("2021-07").toString());

// `overflow` is a real option and its two settings really differ.
console.log(Temporal.PlainDate.from({ year: 2001, month: 2, day: 31 }).toString());
console.log(
	Temporal.PlainDate.from({ year: 2001, month: 2, day: 31 }, { overflow: "constrain" }).toString(),
);

// A field bag for a time, and the wall-clock getters.
const time = Temporal.PlainTime.from({ hour: 9, minute: 5, second: 3 });
console.log(time.toString(), time.hour, time.minute, time.second, time.millisecond);

// Durations come from their own bag and report their own components.
const span = Temporal.Duration.from({ hours: 3, minutes: 30 });
console.log(span.toString(), span.hours, span.minutes, span.sign, span.blank);

// Ordering, which is a static on each type rather than a comparison operator.
console.log(Temporal.PlainDate.compare("2020-01-01", "2021-01-01"));
console.log(Temporal.PlainDate.compare(date, date));
console.log(Temporal.PlainDate.compare({ year: 2030, month: 1, day: 1 }, "2021-01-01"));

// The type keeps its identity as far as both engines agree it does. `typeof` is "object" in both.
//
// Three neighbouring questions are deliberately *not* asked here, because the two engines answer
// them differently and the native answer is the correct one -- checked against `deno`, which has
// `Temporal` where `node` does not:
//
//   Object.prototype.toString.call(date)  spec/deno/native: "[object Temporal.PlainDate]"
//                                         interpreter:      "[object Object]"
//   date.constructor === Temporal.PlainDate  spec/deno/native: true   interpreter: false
//   Number(date)                          spec/deno/native: TypeError  interpreter: NaN
//
// Adding them would either fail this corpus or force the backend to reproduce a wrong answer to
// match, and agreeing on a wrong answer is the one outcome this arrangement exists to prevent.
// They belong in the interpreter, which is where the divergence is.
console.log(typeof date);

// A range the calendar refuses, and the class of error it refuses with.
try {
	Temporal.PlainDate.from({ year: 2001, month: 13, day: 1 }, { overflow: "reject" });
	console.log("no throw");
} catch (error) {
	console.log("reject:", error instanceof RangeError);
}

// Malformed ISO text is a different class again, and the two are not interchangeable.
try {
	Temporal.PlainDate.from("not-a-date");
	console.log("no throw");
} catch (error) {
	console.log("parse threw:", error instanceof RangeError || error instanceof SyntaxError);
}

// Arithmetic. The operand of `add` is a duration whichever type the receiver is, and the answer
// keeps the receiver's type.
console.log(date.add({ months: 1, days: 3 }).toString());
console.log(date.subtract({ years: 1 }).toString());
console.log(date.with({ day: 1 }).toString());
console.log(date.equals("2026-08-30"), date.equals("2026-08-31"));

// A difference is a duration, and `until` and `since` are not each other's negation in general --
// they round in opposite directions -- so both are asked.
console.log(date.until("2027-03-20").toString());
console.log(date.since("2019-01-01").toString());
console.log(date.until("2027-03-20", { largestUnit: "year" }).toString());

// Conversions, including the two that mean different things on different types: `toPlainDate`
// drops a component on a date-and-time and supplies one on a year-month.
console.log(date.toPlainYearMonth().toString(), date.toPlainMonthDay().toString());
console.log(date.toPlainDateTime().toString());
console.log(date.toPlainDateTime(Temporal.PlainTime.from("10:30")).toString());
console.log(date.toZonedDateTime("UTC").toString());
console.log(Temporal.PlainYearMonth.from("2021-07").toPlainDate({ day: 4 }).toString());
console.log(Temporal.PlainMonthDay.from("07-04").toPlainDate({ year: 2000 }).toString());

// A wider value narrows through `from`, which is `ToTemporalDate` rather than a conversion this
// backend could get wrong: the engine decides which pairs exist.
const stamp = Temporal.PlainDateTime.from("2020-01-15T10:20:30.456");
console.log(Temporal.PlainDate.from(stamp).toString(), Temporal.PlainTime.from(stamp).toString());
console.log(stamp.toPlainDate().toString(), stamp.toPlainTime().toString());

// Rounding, in both the shorthand and the full-options spelling, and `toString`'s own rounding --
// which is a different operation with a different default.
console.log(stamp.round("hour").toString());
console.log(stamp.round({ smallestUnit: "minute", roundingMode: "floor" }).toString());
console.log(stamp.toString({ fractionalSecondDigits: 1 }));
console.log(stamp.toString({ smallestUnit: "second" }));

// Durations: the two unary operations, the one that answers a bare number, and a merge.
console.log(span.negated().toString(), span.abs().toString());
console.log(span.add({ minutes: 45 }).toString());
console.log(span.total("minutes"), span.total({ unit: "hours" }));
console.log(span.round({ smallestUnit: "hours", roundingMode: "ceil" }).toString());
console.log(span.with({ hours: 1 }).toString());

// An instant and a zoned date-and-time, where the zone is part of the value rather than a
// formatting choice.
const instant = Temporal.Instant.from("2020-01-15T10:20:30Z");
console.log(instant.add({ hours: 2 }).toString());
console.log(instant.until("2020-02-01T00:00:00Z").toString());
console.log(instant.toZonedDateTimeISO("UTC").toString());
console.log(instant.round({ smallestUnit: "minute" }).toString());

const zoned = Temporal.ZonedDateTime.from("2020-01-15T10:20:30+00:00[UTC]");
console.log(zoned.startOfDay().toString());
console.log(zoned.withTimeZone("UTC").toString());
console.log(zoned.withPlainTime(Temporal.PlainTime.from("01:02")).toString());
console.log(zoned.toPlainDate().toString(), zoned.toInstant().toString());
console.log(zoned.add({ days: 1 }).toString());

// The error classes the arithmetic surface distinguishes. `Temporal` picks between them on nearly
// every path, and collapsing any two would change which `catch` a program takes.
try {
	(date as unknown as { add: () => void }).add();
	console.log("no throw");
} catch (error) {
	console.log("add without an operand:", error instanceof TypeError);
}
try {
	date.add({ months: 1 }, { overflow: "sideways" } as unknown as { overflow: "constrain" });
	console.log("no throw");
} catch (error) {
	console.log("a bogus overflow:", error instanceof RangeError);
}
try {
	(zoned as unknown as { startOfDay: () => void }).startOfDay.call(date);
	console.log("no throw");
} catch (error) {
	console.log("startOfDay on a date:", error instanceof TypeError);
}

// Every option is read, in the specified order, whichever ones the operation uses -- the reads are
// observable and the specification runs them all, so reading fewer would hide a getter.
const witnessed: string[] = [];
const watched: Record<string, unknown> = {};
for (const option of [
	"calendarName",
	"disambiguation",
	"fractionalSecondDigits",
	"largestUnit",
	"offset",
	"overflow",
	"relativeTo",
	"roundingIncrement",
	"roundingMode",
	"smallestUnit",
	"timeZoneName",
]) {
	Object.defineProperty(watched, option, {
		get() {
			witnessed.push(option);
			return undefined;
		},
		enumerable: true,
	});
}
console.log(date.add({ days: 1 }, watched).toString());
console.log(witnessed.join(","));

// Each of these is a function the program calls, and its `name` is the bare method name.
console.log(date.add.name, date.until.name, date.toPlainYearMonth.name);
console.log(typeof date.add, typeof Temporal.Now.instant);

// `Temporal.Now` reads the realm's clock, which ADR 0015 makes virtual, UTC, and started at the
// epoch -- so its answer is a build-reproducible constant rather than the wall time. It arrives in
// whole nanoseconds: `Temporal` measures instants in them, and reading the clock through the
// millisecond accessor would round it and put the two engines a fraction apart.
console.log(Temporal.Now.timeZoneId());
console.log(Temporal.Now.instant().toString(), Date.now());
console.log(Temporal.Now.plainDateISO().toString(), Temporal.Now.plainTimeISO().toString());
console.log(Temporal.Now.plainDateTimeISO().toString());
console.log(Temporal.Now.zonedDateTimeISO("UTC").toString());

// The clock advances with the realm's timers and with nothing else, so two reads with a timer
// between them differ by exactly the delay.
setTimeout(() => {
	console.log(Temporal.Now.instant().toString(), Date.now());
	console.log(Temporal.Now.instant().epochMilliseconds === Date.now());
}, 1500);
