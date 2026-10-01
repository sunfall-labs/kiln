// kiln eval smoke test (authored for kiln, not from porffor).
import { Keyv } from 'keyv';

const kv = new Keyv();
await kv.set('name', 'ada');
console.log(await kv.get('name'));
await kv.set('obj', { a: 1 });
console.log(JSON.stringify(await kv.get('obj')));
console.log(await kv.has('name'));
await kv.delete('name');
console.log(await kv.get('name'), await kv.has('name'));
console.log(kv.namespace);
