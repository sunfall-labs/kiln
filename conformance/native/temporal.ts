// `Temporal` under both engines, through the one linked provider (ADR 0025, ADR 0029 §4).
//
// The engine is `kiln-temporal` over `temporal_rs` in both cases -- the interpreter links the
// crate and the native artifact links the same crate through the provider archive -- so a
// disagreement here is the object model around it, not a calendar.
//
// Only the operations the native backend actually carries are exercised. The arithmetic and
// conversion surface (`add`, `until`, `round`, `with`, `equals`, `toPlainDate`) refuses by name
// under `--native`, and a probe that printed several lines and then refused would read as a
// divergence rather than as the refusal it is.

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

// `Temporal.Now` reads the realm's clock, which ADR 0015 makes virtual and UTC. The zone is
// answerable without reading it; the five instant readers are not, and refuse by name -- the
// clock this backend reaches carries milliseconds where the interpreter feeds the engine
// nanoseconds, and wiring them from it would disagree below the millisecond rather than refuse.
console.log(Temporal.Now.timeZoneId());
