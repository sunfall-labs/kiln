import dayjs from "dayjs";

// The reason dayjs is in this corpus: `diff` subtracts two wrappers, so the whole
// library depends on `-` reducing an object through `valueOf`.
const a = dayjs("2020-01-01T00:00:00Z");
const b = dayjs("2021-03-05T12:30:00Z");
console.log(b.diff(a, "day"), b.diff(a, "hour"), a < b, +a);
console.log(a.add(1, "year").toISOString(), b.subtract(2, "month").toISOString());
console.log(a.isBefore(b), b.isAfter(a), a.isSame(dayjs(a)));
console.log(`${a.toISOString()}`, String(a.valueOf()));
