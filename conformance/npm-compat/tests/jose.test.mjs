// kiln eval smoke test (authored for kiln, not from porffor).
import {SignJWT, jwtVerify, decodeJwt} from 'jose';
const key = new TextEncoder().encode('secret-key-32-bytes-long-12345678');
const jwt = await new SignJWT({sub: '123', role: 'admin'})
  .setProtectedHeader({alg: 'HS256'})
  .sign(key);
console.log(jwt.split('.').length);
console.log(JSON.stringify(decodeJwt(jwt)));
const {payload} = await jwtVerify(jwt, key);
console.log(payload.sub, payload.role);
console.log(jwt.split('.')[0].length > 10);
