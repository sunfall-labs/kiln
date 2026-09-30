// kiln eval smoke test (authored for kiln, not from porffor).
import {glob} from 'glob';
console.log(JSON.stringify(await glob('test-batch-*.txt')));
console.log(JSON.stringify(await glob('test-batch-1.txt')));
console.log(JSON.stringify(await glob('no-such-file-*.zzz')));
