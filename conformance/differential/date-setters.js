// Every setter mirrors a reader, and an out-of-range part rolls over rather than clamping.
const d = new Date(Date.UTC(2020, 0, 15, 10, 30, 0, 0));
console.log(d.toISOString());

d.setUTCFullYear(2021);
console.log(d.toISOString());
d.setUTCMonth(11);
console.log(d.toISOString());
d.setUTCDate(31);
console.log(d.toISOString());
d.setUTCHours(23, 59, 58, 999);
console.log(d.toISOString());

// Rolling over: month 12 is January of the next year, day 0 is the last of the previous month.
const roll = new Date(Date.UTC(2020, 0, 15));
roll.setUTCMonth(12);
console.log(roll.toISOString());
roll.setUTCDate(0);
console.log(roll.toISOString());

// `setTime` takes the instant directly, and a NaN makes the date invalid for good.
const t = new Date(0);
t.setTime(86400000);
console.log(t.toISOString(), t.getTime());
t.setTime(NaN);
console.log(t.getTime(), String(t.getUTCFullYear()));

// A setter returns the new instant, and mutates in place.
const shared = new Date(0);
const same = shared;
console.log(shared.setUTCFullYear(1999) === same.getTime());

// The local setters are the UTC setters: the realm has one zone.
const local = new Date(0);
local.setFullYear(2000, 5, 6);
console.log(local.toISOString(), local.getTimezoneOffset());
