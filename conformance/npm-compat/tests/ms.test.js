const ms = require('ms');

console.log('--- String to ms ---');
console.log(ms('100'));
console.log(ms('100ms'));
console.log(ms('1.5s'));
console.log(ms('1m'));
console.log(ms('2.5 hrs'));
console.log(ms('1d'));
console.log(ms('2.5 days'));
console.log(ms('1y'));
console.log(ms('-100ms'));
console.log(ms('-1.5h'));

console.log('--- ms to string (short) ---');
console.log(ms(500));
console.log(ms(1200));
console.log(ms(60000));
console.log(ms(3600000));
console.log(ms(86400000));
console.log(ms(-60000));

console.log('--- ms to string (long) ---');
console.log(ms(500, { long: true }));
console.log(ms(1000, { long: true }));
console.log(ms(1200, { long: true }));
console.log(ms(60000, { long: true }));
console.log(ms(3600000, { long: true }));
console.log(ms(86400000, { long: true }));
console.log(ms(172800000, { long: true }));
console.log(ms(-60000, { long: true }));

console.log('--- Invalid inputs ---');
console.log(ms('invalid'));
console.log(ms('10 foo'));
try { ms(''); } catch (e) { console.log('caught empty string'); }
try { ms({}); } catch (e) { console.log('caught object error'); }
try { ms(null); } catch (e) { console.log('caught null error'); }
try { ms(undefined); } catch (e) { console.log('caught undefined error'); }
try { ms(NaN); } catch (e) { console.log('caught nan error'); }
try { ms(Infinity); } catch (e) { console.log('caught infinity error'); }
