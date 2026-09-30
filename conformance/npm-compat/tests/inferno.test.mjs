// kiln eval smoke test (authored for kiln, not from porffor).
import {createVNode, createTextVNode, createFragment, version} from 'inferno';
console.log(version);
const v = createVNode(1, 'div', 'cls', 'hello', null, 'k1', null);
console.log(v.type, v.className, v.children, v.key);
console.log(createTextVNode('text').children);
const frag = createFragment(['a', 'b'], 0, 'fk');
console.log(frag.children.length, frag.key);
