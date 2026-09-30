// kiln eval smoke test (authored for kiln, not from porffor).
const sig = require('cookie-signature');
const signed = sig.sign('session-id-1', 's3cret');
console.log(sig.unsign(signed, 's3cret'));
console.log(sig.unsign(signed, 'wrong'));
console.log(sig.unsign('tampered.sig', 's3cret'));
const s2 = sig.sign('', 'k');
console.log(sig.unsign(s2, 'k') === '');
