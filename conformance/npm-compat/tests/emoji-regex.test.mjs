// kiln eval smoke test (authored for kiln, not from porffor).
import emojiRegex from 'emoji-regex';
const re = emojiRegex();
console.log(re.test('party 🎉 time'));
console.log(re.test('no emoji here'));
console.log('🎉🚀'.match(re).length);
console.log('a😀b'.replace(re, '*'));
console.log(re instanceof RegExp);
