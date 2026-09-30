// kiln eval smoke test (authored for kiln, not from porffor).
const moment = require('moment');

const d = moment('2026-09-30T12:00:00Z');
console.log('format:', d.format('YYYY-MM-DD HH:mm'));
console.log('add:', d.clone().add(5, 'days').format('YYYY-MM-DD'));
console.log('subtract:', d.clone().subtract(2, 'months').format('YYYY-MM'));
console.log('diff days:', moment('2026-10-05').diff(moment('2026-09-30'), 'days'));
console.log('isBefore:', moment('2020-01-01').isBefore(moment('2026-01-01')));
console.log('startOf:', d.clone().startOf('month').format('YYYY-MM-DD'));
console.log('unix:', moment('2026-01-01T00:00:00Z').unix());
console.log('from fixed:', moment('2026-09-29T12:00:00Z').from(moment('2026-09-30T12:00:00Z')));
console.log('isValid:', moment('2026-13-99').isValid() === false);
console.log('duration:', moment.duration(90, 'minutes').asHours());
