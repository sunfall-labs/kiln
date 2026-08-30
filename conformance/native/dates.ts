// Every instant here is written out explicitly. `Date.now()` and a bare `new Date()` are the
// *current* time, which is real in node and virtual here, so they can never agree -- see ADR
// 0015. What can be compared is everything downstream of a stated instant.
console.log(typeof Date.now(), typeof new Date().getTime());

const at = new Date(Date.UTC(2026, 7, 29, 12, 34, 56, 789));
console.log(at.toISOString(), at.getTime());
console.log(at.getUTCFullYear(), at.getUTCMonth(), at.getUTCDate(), at.getUTCDay());
console.log(at.getUTCHours(), at.getUTCMinutes(), at.getUTCSeconds(), at.getUTCMilliseconds());

// The realm is UTC, which `getTimezoneOffset` reports rather than leaving to be discovered.
console.log(at.getFullYear(), at.getMonth(), at.getDate(), at.getTimezoneOffset());

// The epoch, and instants before it -- which is where floor division matters.
console.log(new Date(0).toISOString(), new Date(-1).toISOString(), new Date(-86400000).toISOString());
console.log(new Date(-1).getUTCFullYear(), new Date(-1).getUTCMonth(), new Date(-1).getUTCDate());

// Leap days and century rules, which is what the calendar arithmetic is for.
console.log(new Date(Date.UTC(2024, 1, 29)).toISOString(), new Date(Date.UTC(2000, 1, 29)).toISOString());
console.log(new Date(Date.UTC(1900, 1, 28)).getUTCDay(), new Date(Date.UTC(2100, 2, 1)).toISOString());

// Out-of-range fields roll over rather than failing.
console.log(new Date(Date.UTC(2026, 12, 1)).toISOString(), new Date(Date.UTC(2026, 0, 32)).toISOString());

// Parsing accepts the ISO forms and rejects the rest, rather than guessing.
console.log(Date.parse('2026-08-29T00:00:00.000Z'), Date.parse('2026-08-29'));
console.log(Date.parse('not a date'), new Date('nonsense').getTime());

// An invalid date propagates through every reader and has no ISO spelling.
const invalid = new Date(NaN);
console.log(String(invalid), invalid.getTime(), invalid.getUTCFullYear());
console.log(JSON.stringify({ when: invalid }), JSON.stringify({ when: new Date(0) }));

// A date converts to its instant, which is what makes a difference a duration.
const start = new Date(Date.UTC(2026, 0, 1));
const end = new Date(Date.UTC(2026, 0, 2));
console.log(end - start, +start, start < end, start.valueOf());

// Two dates built from one instant are still two objects.
console.log(new Date(0) === new Date(0), start instanceof Date);

// A Date reduces to its instant for arithmetic and to its human form for text, which is the
// hint distinction the whole ToPrimitive change was about.
const dayOne = new Date(Date.UTC(2026, 0, 1));
const dayTwo = new Date(Date.UTC(2026, 0, 2));
console.log(dayTwo - dayOne, +dayOne, dayOne < dayTwo, `${dayOne}`.slice(0, 3), String(dayOne).slice(0, 3));
console.log(JSON.stringify({ at: dayOne }), JSON.stringify([dayOne]), dayOne.toJSON());
console.log(dayOne instanceof Date, ({}) instanceof Date, [] instanceof Object, dayOne.getTime());
