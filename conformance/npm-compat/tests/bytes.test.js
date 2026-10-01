const bytes = require('bytes');

console.log('--- bytes.format with options ---');
console.log(bytes.format(1024));
console.log(bytes.format(1000));
console.log(bytes.format(1024 * 1024));
console.log(bytes.format(1024 * 1024 * 1024 * 2.5));
console.log(bytes.format(1024 * 1024 * 1024 * 1024 * 5));
console.log(bytes.format(1536, { decimalPlaces: 0 }));
console.log(bytes.format(1536, { decimalPlaces: 3 }));
console.log(bytes.format(1024 * 1024, { unit: 'KB' }));
console.log(bytes.format(1000000000, { thousandsSeparator: ',' }));
console.log(bytes.format(1024, { unitSeparator: '_' }));
console.log(bytes.format(0));
console.log(bytes.format(-1024));

console.log('--- bytes.parse ---');
console.log(bytes.parse('1B'));
console.log(bytes.parse('512B'));
console.log(bytes.parse('1KB'));
console.log(bytes.parse('10.5 MB'));
console.log(bytes.parse('2.25gb'));
console.log(bytes.parse('1tb'));
console.log(bytes.parse('1pb'));
console.log(bytes.parse('256kb'));
console.log(bytes.parse('-1KB'));

console.log('--- bytes function shorthand ---');
console.log(bytes(1024));
console.log(bytes('1.5MB'));

console.log('--- Invalid inputs ---');
console.log(bytes.parse(''));
console.log(bytes.parse('not a byte'));
console.log(bytes.parse(null));
console.log(bytes.parse(undefined));
console.log(bytes.format(NaN));
console.log(bytes.format(Infinity));
