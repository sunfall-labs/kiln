// kiln eval smoke test (authored for kiln, not from porffor).
const log = require('loglevel');
log.setLevel('silent');
console.log(log.getLevel());
log.setLevel('warn');
console.log(log.getLevel());
const other = log.getLogger('other');
other.setLevel('error');
console.log(other.getLevel());
console.log(Object.keys(log.getLoggers()));
