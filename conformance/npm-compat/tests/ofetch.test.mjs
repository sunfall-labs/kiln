// kiln eval smoke test (authored for kiln, not from porffor).
import { $fetch } from 'ofetch';

const api = $fetch.create({ baseURL: 'http://localhost:9', retry: 0 });
console.log(typeof $fetch, typeof api, typeof api.raw);
console.log(typeof $fetch.create);
const withHeaders = $fetch.create({ headers: { 'x-a': '1' } });
console.log(typeof withHeaders);
