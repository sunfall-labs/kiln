// kiln eval smoke test (authored for kiln, not from porffor).
import { compile } from 'svelte/compiler';
import { writable, derived, get } from 'svelte/store';

const result = compile('<h1>Hello {name}!</h1>', { filename: 'demo.svelte' });
console.log(result.js.code.includes('Hello'));
console.log(result.warnings.length);
console.log(typeof result.js.map === 'object');

const count = writable(1);
const values = [];
const unsub = count.subscribe((v) => values.push(v));
count.set(2);
count.update((v) => v * 10);
unsub();
count.set(99);
console.log(JSON.stringify(values));

const doubled = derived(count, ($c) => $c * 2);
console.log(get(doubled));
