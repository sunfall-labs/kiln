// kiln eval smoke test (authored for kiln, not from porffor).
import prettyBytes from 'pretty-bytes';

console.log(prettyBytes(100));
console.log(prettyBytes(1337));
console.log(prettyBytes(1337, { binary: true }));
console.log(prettyBytes(0));
console.log(prettyBytes(-5000000));
