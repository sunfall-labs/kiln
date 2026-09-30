// kiln eval smoke test (authored for kiln, not from porffor).
import ky from 'ky';

console.log(typeof ky, typeof ky.extend);
const api = ky.extend({ prefixUrl: 'http://localhost:9', timeout: 500, retry: 0 });
console.log(typeof api, typeof api.get, typeof api.post);
console.log(typeof ky.get, typeof ky.head);
const api2 = api.extend({ headers: { 'x-test': '1' } });
console.log(typeof api2.get);
