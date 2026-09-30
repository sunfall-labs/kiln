const { KeyObject, createHmac, createSecretKey } = require('node:crypto');
const key = createSecretKey(Buffer.from('key'));
console.log(key instanceof KeyObject, key.type, key.symmetricKeySize);
console.log(createHmac('sha256', key).update('data').digest('hex'));
const stringKey = createSecretKey('6b6579', 'hex');
console.log(createHmac('sha256', stringKey).update('data').digest('hex'));
