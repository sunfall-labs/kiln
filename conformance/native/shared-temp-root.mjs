import {share} from 'kiln:shared';
console.log('two', share({}) !== share({}));
const first=share({}); const second=share({});
console.log('vars', first !== second);
