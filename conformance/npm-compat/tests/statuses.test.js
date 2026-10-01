const statuses = require('statuses');

console.log('--- Code lookups ---');
console.log(statuses(200));
console.log(statuses(201));
console.log(statuses(204));
console.log(statuses(301));
console.log(statuses(304));
console.log(statuses(400));
console.log(statuses(401));
console.log(statuses(403));
console.log(statuses(404));
console.log(statuses(418));
console.log(statuses(429));
console.log(statuses(500));
console.log(statuses(502));
console.log(statuses(503));

console.log('--- Message lookups (case-insensitive) ---');
console.log(statuses('ok'));
console.log(statuses('OK'));
console.log(statuses('Not Found'));
console.log(statuses('not found'));
console.log(statuses('TOO MANY REQUESTS'));
console.log(statuses("i'm a teapot"));

console.log('--- Categorical mappings ---');
console.log('204 empty:', Boolean(statuses.empty[204]));
console.log('304 empty:', Boolean(statuses.empty[304]));
console.log('200 empty:', Boolean(statuses.empty[200]));
console.log('301 redirect:', Boolean(statuses.redirect[301]));
console.log('307 redirect:', Boolean(statuses.redirect[307]));
console.log('404 redirect:', Boolean(statuses.redirect[404]));
console.log('503 retry:', Boolean(statuses.retry[503]));
console.log('500 retry:', Boolean(statuses.retry[500]));

console.log('--- codes array ---');
console.log('is array:', Array.isArray(statuses.codes));
console.log('has 200:', statuses.codes.includes(200));
console.log('has 404:', statuses.codes.includes(404));

console.log('--- Invalid inputs ---');
try { statuses(999); } catch (e) { console.log('caught 999'); }
try { statuses('nonexistent status message'); } catch (e) { console.log('caught nonexistent'); }
