// kiln npm functional test: dayjs
const dayjs = require('dayjs');

// 1. Parsing diverse inputs & validation
console.log('iso:', dayjs('2025-06-15T12:30:45.000Z').toISOString());
console.log('millis:', dayjs(1700000000000).format('YYYY-MM-DD'));
console.log('date-obj:', dayjs(new Date(2024, 0, 1)).format('YYYY-MM-DD'));
console.log('invalid:', dayjs('not-a-date').isValid());

// 2. Arithmetic & Leap Year Edge Cases
const jan31 = dayjs('2024-01-31');
console.log('leap-feb-add-month:', jan31.add(1, 'month').format('YYYY-MM-DD'));
const nonLeapJan31 = dayjs('2023-01-31');
console.log('nonleap-feb-add-month:', nonLeapJan31.add(1, 'month').format('YYYY-MM-DD'));

const math = dayjs('2024-05-10')
  .add(2, 'year')
  .subtract(3, 'month')
  .add(15, 'day');
console.log('chained-arithmetic:', math.format('YYYY-MM-DD'));

// 3. Diffing across units
const d1 = dayjs('2024-01-01');
const d2 = dayjs('2024-04-01');
console.log('diff-months:', d2.diff(d1, 'month'));
console.log('diff-days:', d2.diff(d1, 'day'));
console.log('diff-weeks:', Math.floor(d2.diff(d1, 'week', true)));

// 4. Boundary queries: startOf & endOf
const midYear = dayjs('2024-07-15T15:30:00.000Z');
console.log('startOf-year:', midYear.startOf('year').format('YYYY-MM-DD'));
console.log('endOf-month:', midYear.endOf('month').format('YYYY-MM-DD'));

// 5. Comparison queries
console.log('isBefore:', dayjs('2024-01-01').isBefore(dayjs('2024-01-02')));
console.log('isAfter:', dayjs('2024-01-02').isAfter(dayjs('2024-01-01')));
console.log('isSame-day:', dayjs('2024-01-01T10:00:00').isSame('2024-01-01T20:00:00', 'day'));

// 6. Getters and Setters
const mutableBase = dayjs('2024-03-14');
console.log('getters:', mutableBase.year(), mutableBase.month(), mutableBase.date());
const modified = mutableBase.set('year', 2030).set('month', 11).set('date', 25);
console.log('setters:', modified.format('YYYY-MM-DD'));
