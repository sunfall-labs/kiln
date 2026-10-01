// kiln eval smoke test (authored for kiln, not from porffor).
import prettyMs from 'pretty-ms';

console.log(prettyMs(1337));
console.log(prettyMs(60000));
console.log(prettyMs(86400000 + 3600000));
console.log(prettyMs(0));
console.log(prettyMs(500, { verbose: true }));
