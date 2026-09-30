// kiln eval smoke test (authored for kiln, not from porffor).
const { CronExpressionParser } = require('cron-parser');
const base = { currentDate: new Date('2024-01-01T00:00:00Z') };
console.log(CronExpressionParser.parse('*/15 * * * *', base).next().toISOString());
console.log(CronExpressionParser.parse('0 12 * * MON', { ...base, tz: 'UTC' }).next().toISOString());
console.log(CronExpressionParser.parse('0 9 * * 1', base).next().toISOString());
console.log(CronExpressionParser.parse('0 0 1 1 *', base).next().toISOString());
