// kiln eval smoke test (authored for kiln, not from porffor).
import Long from 'long';

console.log(Long.fromNumber(123456789).toString());
console.log(Long.fromString('9007199254740993').add(Long.ONE).toString());
console.log(Long.MAX_UNSIGNED_VALUE.toString());
console.log(Long.fromNumber(-42).neg().toString());
console.log(Long.fromBits(0xffffffff, 0x7fffffff).toString());
console.log(Long.fromNumber(10).multiply(Long.fromNumber(20)).toString());
