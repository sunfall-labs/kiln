// kiln eval smoke test (authored for kiln, not from porffor).
const { h, Fragment, Component, cloneElement, createElement, toChildArray } = require('preact');

const vnode = h('div', { id: 'app', className: 'x' }, 'hello', h('span', null, 'world'));
console.log('type:', vnode.type, 'props.id:', vnode.props.id);
console.log('children count:', vnode.props.children.length);

const frag = h(Fragment, null, h('li', null, 'a'), h('li', null, 'b'));
console.log('fragment type is Fragment:', frag.type === Fragment);

class Greet extends Component {
  render() { return h('p', null, 'hi ' + this.props.name); }
}
const inst = h(Greet, { name: 'kiln' });
console.log('component type:', inst.type === Greet, 'props:', JSON.stringify(inst.props));

const cloned = cloneElement(vnode, { id: 'cloned' });
console.log('cloneElement:', cloned.props.id, vnode.props.id);

console.log('createElement === h:', createElement === h);
const kids = toChildArray(h('div', null, 'a', 'b', null, 'c'));
console.log('toChildArray:', JSON.stringify(kids));

const keyed = h('li', { key: 'k1' }, 'x');
console.log('key:', keyed.key);
