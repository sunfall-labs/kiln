// kiln eval smoke test (authored for kiln, not from porffor).
import { getProperty, setProperty, deleteProperty, hasProperty } from 'dot-prop';
const o = { a: { b: 1 } };
console.log(getProperty(o, 'a.b'));
console.log(hasProperty(o, 'a.b'), hasProperty(o, 'a.zzz'));
setProperty(o, 'a.c', 2);
console.log(JSON.stringify(o));
deleteProperty(o, 'a.b');
console.log(JSON.stringify(o));
console.log(getProperty(o, 'missing.path', 'fallback'));
