// kiln eval smoke test (authored for kiln, not from porffor).
const CryptoJS = require('crypto-js');
console.log(CryptoJS.MD5('hello').toString());
console.log(CryptoJS.SHA256('hello').toString().slice(0, 16));
const enc = CryptoJS.AES.encrypt('secret message', 'key123');
console.log(CryptoJS.AES.decrypt(enc, 'key123').toString(CryptoJS.enc.Utf8));
console.log(CryptoJS.HmacSHA1('msg', 'k').toString().slice(0, 12));
console.log(CryptoJS.SHA1('abc').toString());
