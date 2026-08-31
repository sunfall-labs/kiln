// There is no local time zone: kiln is always UTC.
//
// `getTimezoneOffset()` answers 0 and `Intl.DateTimeFormat().resolvedOptions().timeZone` answers
// "UTC" whatever the host is set to, so every local-time getter returns the UTC value. date-fns
// `format` is local-time by construction, which is why conformance/thirdparty/datefns prints
// 2020-02-15 where node prints 2020-02-14 -- one day off, silently.
//
// It is the worst shape a bug can take for a test suite: correct under CI, where TZ is UTC, and
// wrong on any developer machine or production host west of Greenwich.
//
// node (TZ=America/Denver):  offset 420  local 2020 2 14 17  tz America/Denver
// kiln:                      offset 0    local 2020 2 15 0   tz UTC
const d = new Date("2020-02-15T00:00:00.000Z");
console.log("iso", d.toISOString());
console.log("offset", d.getTimezoneOffset());
console.log("local", d.getFullYear(), d.getMonth() + 1, d.getDate(), d.getHours());
console.log("utc", d.getUTCFullYear(), d.getUTCMonth() + 1, d.getUTCDate(), d.getUTCHours());
console.log("tz", Intl.DateTimeFormat().resolvedOptions().timeZone);
console.log("string", d.toString().slice(0, 33));
