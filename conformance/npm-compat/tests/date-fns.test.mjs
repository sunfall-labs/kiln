import {
  format,
  parseISO,
  addDays,
  subDays,
  addMonths,
  subMonths,
  addYears,
  isAfter,
  isBefore,
  isEqual,
  isLeapYear,
  isWeekend,
  differenceInDays,
  differenceInMonths,
  startOfMonth,
  endOfMonth,
  min,
  max
} from 'date-fns';

console.log('--- Formatting & Parsing ---');
const d = new Date(2024, 0, 15, 12, 0, 0);
console.log(format(d, 'yyyy-MM-dd'));
console.log(format(addDays(d, 5), 'yyyy-MM-dd'));
console.log(format(subDays(d, 5), 'yyyy-MM-dd'));
console.log(format(addMonths(d, 2), 'yyyy-MM-dd'));
console.log(format(addYears(d, 1), 'yyyy-MM-dd'));

console.log('--- Leap year & Weekend tests ---');
console.log('isLeapYear 2024:', isLeapYear(new Date(2024, 1, 1)));
console.log('isLeapYear 2023:', isLeapYear(new Date(2023, 1, 1)));
console.log('isWeekend Saturday (Jan 13):', isWeekend(new Date(2024, 0, 13)));
console.log('isWeekend Monday (Jan 15):', isWeekend(d));

console.log('--- Differences and Comparisons ---');
const dStart = new Date(2024, 0, 1);
const dEnd = new Date(2024, 3, 1);
console.log('differenceInDays:', differenceInDays(dEnd, dStart));
console.log('differenceInMonths:', differenceInMonths(dEnd, dStart));
console.log('isAfter:', isAfter(dEnd, dStart));
console.log('isBefore:', isBefore(dStart, dEnd));
console.log('isEqual:', isEqual(dStart, new Date(2024, 0, 1)));

console.log('--- Month boundaries & Extremes ---');
console.log('startOfMonth:', format(startOfMonth(d), 'yyyy-MM-dd'));
console.log('endOfMonth:', format(endOfMonth(d), 'yyyy-MM-dd'));
console.log('min date is dStart:', isEqual(min([dStart, d, dEnd]), dStart));
console.log('max date is dEnd:', isEqual(max([dStart, d, dEnd]), dEnd));
