// kiln eval smoke test (authored for kiln, not from porffor).
const { h } = require('preact');
const render = require('preact-render-to-string');

console.log(render(h('div', null, 'hi')));
console.log(render(h('span', { class: 'x' }, 'a', 'b')));
console.log(render(h('input', { type: 'text', value: 'q', disabled: true })));

const Cmp = ({ name }) => h('p', null, `Hello, ${name}!`);
console.log(render(h(Cmp, { name: 'World' })));

console.log(render(h('ul', null, [1, 2, 3].map((n) => h('li', { key: n }, n)))));
