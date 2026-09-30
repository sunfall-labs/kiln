// kiln eval smoke test (authored for kiln, not from porffor).
const { format, register, cancel } = require('timeago.js');

const now = new Date('2026-01-01T00:00:00Z');
const opts = { relativeDate: now };

console.log(format(new Date('2025-12-31T23:59:50Z'), 'en_US', opts));
console.log(format(new Date('2025-12-31T23:00:00Z'), 'en_US', opts));
console.log(format(new Date('2025-12-25T00:00:00Z'), 'en_US', opts));
console.log(format(new Date('2025-01-01T00:00:00Z'), 'zh_CN', opts));

register('test_locale', (number, index) => [`${number} units ago`, 'now'][index > 0 ? 0 : 1]);
console.log(format(new Date('2025-12-31T12:00:00Z'), 'test_locale', opts));
console.log(typeof cancel);
