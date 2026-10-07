// kiln-native: profile=native-release
const keyBytes = new TextEncoder().encode('secret-key-32-bytes-long-12345678');
const data = new TextEncoder().encode('kiln release WebCrypto probe');
const expectedDigest = '67f74accfb09e9b511c2d4766824ff9887a77126b8d6989fade97121505397e4';
const expectedHmac = '85195294d7d9fa332d461b65c8a45946be94b7acb3276dc7de0269fe38f09a65';
function hex(buffer) {
  const bytes = new Uint8Array(buffer);
  let result = '';
  for (let index = 0; index < bytes.length; index++) {
    const part = bytes[index].toString(16);
    result += part.length === 1 ? `0${part}` : part;
  }
  return result;
}
const digest = await crypto.subtle.digest('SHA-256', data);
const key = await crypto.subtle.importKey(
  'raw', keyBytes, { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
const signature = await crypto.subtle.sign('HMAC', key, data);
console.log('digest:', hex(digest) === expectedDigest);
console.log('signature:', hex(signature) === expectedHmac);
console.log('verify:', await crypto.subtle.verify('HMAC', key, signature, data));
console.log('reject-tampered:', !(await crypto.subtle.verify(
  'HMAC', key, signature, new TextEncoder().encode('tampered'))));
