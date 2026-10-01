// kiln eval smoke test (authored for kiln, not from porffor).
const { formatInTimeZone } = require('date-fns-tz');
const d = new Date('2024-06-15T12:00:00Z');
console.log(formatInTimeZone(d, 'America/Denver', 'yyyy-MM-dd HH:mm'));
console.log(formatInTimeZone(d, 'Asia/Tokyo', 'yyyy-MM-dd HH:mm'));
console.log(formatInTimeZone(d, 'UTC', "yyyy-MM-dd'T'HH:mm:ss'Z'"));
const { getTimezoneOffset } = require('date-fns-tz');
console.log(getTimezoneOffset('America/Denver', d));
