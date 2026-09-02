// `new Temporal.X(...)`, which is not `Temporal.X.from({...})` and must not be routed through it.
//
// A constructor takes *ISO* components positionally and rejects one out of range. `from` reads
// named fields, resolves them through the calendar, and honours an `overflow` option that
// defaults to `constrain`. The two therefore disagree about the same numbers on purpose:
// `new Temporal.PlainDate(2000, 300, 1)` is a RangeError where the `from` of those numbers is
// 2000-12-01. Both engines reach the same linked engine (ADR 0025), so a disagreement here is the
// object model around it -- and the native side used to refuse every one of the eight.

const T = Temporal;
console.log(new T.PlainDate(2020, 1, 1).toString());
console.log(new T.PlainTime(1, 2, 3).toString(), new T.PlainTime().toString());
console.log(new T.PlainDateTime(2020, 1, 1, 3, 4, 5, 6, 7, 8).toString());
console.log(new T.PlainDateTime(2020, 2, 29).toString());
console.log(new T.PlainYearMonth(2020, 1).toString(), new T.PlainMonthDay(2, 29).toString());
console.log(new T.Duration(1, 2, 3, 4, 5, 6, 7).toString(), new T.Duration().toString());
console.log(new T.Instant(0n).toString(), new T.Instant(1_000_000_000n).toString());
console.log(new T.ZonedDateTime(0n, "UTC").toString());

// The explicit ISO calendar is the same value as the default, and says so.
console.log(new T.PlainDate(2020, 1, 1, "iso8601").toString());

// A leap day through the constructor, which is where a reimplemented calendar would drift.
console.log(new T.PlainDate(2024, 2, 29).toString());

// What the constructor rejects and `from` constrains. Each is a `RangeError` the program can
// catch, not a fault -- so the name of the error is part of the answer.
for (const build of [
  () => new T.PlainDate(2000, 300, 1),
  () => new T.PlainDate(2000, 1, 99),
  () => new T.PlainTime(99),
  () => new T.PlainDateTime(2020, 13, 1),
  () => new T.PlainMonthDay(13, 1),
]) {
  try {
    console.log("built", build().toString());
  } catch (error) {
    console.log("rejected", (error as Error).name);
  }
}
// ...and the same numbers through `from`, which constrains where the constructor rejects.
console.log(T.PlainDate.from({ year: 2020, month: 2, day: 31 }).toString());
try {
  console.log("built", new T.PlainDate(2020, 2, 31).toString());
} catch (error) {
  console.log("rejected", (error as Error).name);
}

// The two exact types take a BigInt and nothing else: nanoseconds pass through a double exactly
// only near 1970, so a number is refused rather than rounded.
try {
  console.log("built", new (T.Instant as unknown as new (value: number) => unknown)(0));
} catch (error) {
  console.log("rejected", (error as Error).name);
}

// Constructed and `from`-ed values are the same values, which is what `equals` is for.
// Constructed and `from`-ed values are the same values, which is what `equals` is for -- and for
// a `Duration`, which the language gives no `equals`, the ISO text is the comparison.
console.log(new T.PlainDate(2020, 6, 15).equals(T.PlainDate.from("2020-06-15")));
console.log(new T.Duration(0, 0, 0, 0, 2).toString() === T.Duration.from({ hours: 2 }).toString());
