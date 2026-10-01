const sig = require('cookie-signature');

console.log('--- sign & unsign ---');
const s1 = sig.sign('session-id-12345', 'secret-key');
console.log('s1 ends with signature:', s1.startsWith('session-id-12345.'));
console.log('s1 unsign valid:', sig.unsign(s1, 'secret-key'));
console.log('s1 unsign wrong secret:', sig.unsign(s1, 'wrong-key'));

console.log('--- value with dots and unicode ---');
const complexVal = 'user.name.with.dots:🚀🔥';
const sComplex = sig.sign(complexVal, 'unicode-secret');
console.log('complex unsign:', sig.unsign(sComplex, 'unicode-secret'));

console.log('--- empty value ---');
const sEmpty = sig.sign('', 'empty-secret');
console.log('empty sign:', sEmpty);
console.log('empty unsign:', sig.unsign(sEmpty, 'empty-secret') === '');

console.log('--- malformed and edge cases ---');
console.log('no dot:', sig.unsign('nodotstring', 'secret'));
console.log('tampered value:', sig.unsign(s1.replace('12345', '12346'), 'secret-key'));
console.log('truncated sig:', sig.unsign(s1.slice(0, -3), 'secret-key'));
console.log('empty input:', sig.unsign('', 'secret'));
try { sig.sign(123, 'k'); } catch (e) { console.log('caught invalid val type'); }
try { sig.sign('val', null); } catch (e) { console.log('caught invalid secret'); }
