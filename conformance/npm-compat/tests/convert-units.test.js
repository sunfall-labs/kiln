// kiln eval smoke test (authored for kiln, not from porffor).
const convert = require('convert-units');
console.log(convert(5).from('km').to('mi'));
console.log(convert(2).from('l').to('ml'));
console.log(convert(32).from('F').to('C'));
console.log(convert(1).from('lb').to('kg'));
console.log(convert().describe('kg').measure);
