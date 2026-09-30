// kiln eval smoke test (authored for kiln, not from porffor).
import {filesize} from 'filesize';
console.log(filesize(1024));
console.log(filesize(123456789));
console.log(filesize(0));
console.log(filesize(1500, {round: 1}));
console.log(filesize(1048576));
