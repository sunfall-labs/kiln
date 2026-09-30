// kiln eval smoke test (authored for kiln, not from porffor).
const jwt = require('jsonwebtoken');
const secret = 'test-secret';
const token = jwt.sign({ sub: '123', role: 'admin' }, secret, { noTimestamp: true });
console.log(token);
console.log(JSON.stringify(jwt.verify(token, secret)));
try { jwt.verify(token, 'wrong'); } catch (e) { console.log(e.name); }
const decoded = jwt.decode(token);
console.log(decoded.sub, decoded.iat === undefined);
