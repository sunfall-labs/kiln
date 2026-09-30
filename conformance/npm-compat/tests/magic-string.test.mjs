// kiln eval smoke test (authored for kiln, not from porffor).
import MagicString from 'magic-string';

const s = new MagicString('hello world');
s.overwrite(0, 5, 'goodbye');
console.log(s.toString());
const s2 = new MagicString('abcdef');
s2.remove(1, 3);
console.log(s2.toString());
const s3 = new MagicString('x = 1;');
s3.appendLeft(0, '/* top */ ');
console.log(s3.toString());
console.log(s.hasChanged(), new MagicString('same').hasChanged());
