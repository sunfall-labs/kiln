// kiln eval smoke test (authored for kiln, not from porffor).
import { trimNewlines } from 'trim-newlines';

console.log(JSON.stringify(trimNewlines('\n\nhello\n\n')));
console.log(JSON.stringify(trimNewlines('no newlines')));
console.log(JSON.stringify(trimNewlines('\r\n\r\nx\r\n')));
console.log(JSON.stringify(trimNewlines('🦄\n\n')));
