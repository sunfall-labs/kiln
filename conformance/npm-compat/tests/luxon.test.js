// kiln eval smoke test (authored for kiln, not from porffor).
const { DateTime, Duration, Interval } = require('luxon');

const dt = DateTime.fromISO('2026-09-30T12:34:56Z', { zone: 'utc' });
console.log('toFormat:', dt.toFormat('yyyy-MM-dd HH:mm'));
console.log('plus:', dt.plus({ days: 3 }).toISODate());
console.log('minus:', dt.minus({ hours: 2 }).toFormat('HH:mm'));
console.log('diff days:', DateTime.fromISO('2026-10-05').diff(DateTime.fromISO('2026-09-30'), 'days').days);
console.log('weekday:', dt.weekdayLong, 'ordinal:', dt.ordinal);
console.log('zone:', dt.zoneName, 'offset:', dt.offset);

const dur = Duration.fromObject({ hours: 2, minutes: 30 });
console.log('duration as minutes:', dur.as('minutes'));
console.log('duration toISO:', dur.toISO());

const iv = Interval.fromDateTimes(DateTime.fromISO('2026-09-01'), DateTime.fromISO('2026-10-01'));
console.log('interval days:', iv.length('days'), 'contains:', iv.contains(dt));
console.log('invalid:', DateTime.fromISO('not-a-date').isValid === false);
console.log('fromMillis:', DateTime.fromMillis(0, { zone: 'utc' }).toISODate());
