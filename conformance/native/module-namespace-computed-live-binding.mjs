import * as namespace from './fixtures/module-namespace-computed-live-binding-source.mjs';
import { update } from './fixtures/module-namespace-computed-live-binding-source.mjs';

const key = 'live';
console.log(namespace.live);
console.log(namespace[key]);
update(42);
console.log(namespace.live);
console.log(namespace[key]);
