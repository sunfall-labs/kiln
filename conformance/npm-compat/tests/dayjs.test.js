// kiln eval smoke test (authored for kiln, not from porffor).
const dayjs = require('dayjs');
console.log(dayjs('2024-01-15').format('YYYY-MM-DD dddd'));
console.log(dayjs('2024-01-15').add(10, 'day').format('YYYY-MM-DD'));
console.log(dayjs('2024-02-01').diff(dayjs('2024-01-15'), 'day'));
console.log(dayjs('2024-01-15').isBefore('2024-01-16'));
console.log(dayjs('2024-01-15').startOf('month').format('YYYY-MM-DD'));
