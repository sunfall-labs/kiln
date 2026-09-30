// kiln eval smoke test (authored for kiln, not from porffor).
import stringWidth from 'string-width';
console.log(stringWidth('hello'));
console.log(stringWidth('古'));
console.log(stringWidth('👍'));
console.log(stringWidth('\u001B[31mred\u001B[39m'));
console.log(stringWidth('a古b'));
