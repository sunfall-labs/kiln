// kiln eval smoke test (authored for kiln, not from porffor).
import {ulid, decodeTime, encodeTime, monotonicFactory} from 'ulid';
const id = ulid(1000);
console.log(id.length);
console.log(decodeTime(id));
console.log(encodeTime(0, 10));
console.log(/^[0-9A-HJKMNP-TV-Z]{26}$/.test(ulid(999)));
const mono = monotonicFactory();
const m1 = mono(5000);
const m2 = mono(5000);
console.log(decodeTime(m1), decodeTime(m2), m1 < m2);
