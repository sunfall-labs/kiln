// kiln eval smoke test (authored for kiln, not from porffor).
import axios from 'axios';
const inst = axios.create({baseURL: 'https://example.com/api', timeout: 1000});
console.log(inst.defaults.baseURL, inst.defaults.timeout);
console.log(axios.getUri({url: 'https://example.com/search', params: {q: 'x', n: 2}}));
console.log(axios.isAxiosError(new Error('nope')), axios.isAxiosError({isAxiosError: true}));
const err = new axios.AxiosError('fail', 'E_X', {url: 'u'}, {}, {});
console.log(err.code, err.message);
console.log(typeof axios.CancelToken);
