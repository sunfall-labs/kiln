// kiln eval smoke test (authored for kiln, not from porffor).
import { customRandom, nanoid } from 'nanoid';

const deterministic = (size) => { const b = new Uint8Array(size); for (let i = 0; i < size; i++) b[i] = i; return b; };
const id10 = customRandom('abcdef', 10, deterministic);
console.log(id10());
console.log(id10());
console.log(nanoid().length);
console.log(nanoid(10).length);
