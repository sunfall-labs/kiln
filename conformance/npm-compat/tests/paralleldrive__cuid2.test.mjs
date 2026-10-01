// kiln eval smoke test (authored for kiln, not from porffor).
import {createId, isCuid} from '@paralleldrive/cuid2';
const id = createId();
console.log(id.length);
console.log(/^[0-9a-z]+$/.test(id));
console.log(isCuid('ckopqme0000000000000000000'));
console.log(createId(10).length);
console.log(isCuid(id));
