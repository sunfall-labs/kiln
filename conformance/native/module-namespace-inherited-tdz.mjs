import { observed } from './fixtures/module-namespace-inherited-tdz-cycle.mjs';

export let value = 1;
console.log(observed);
