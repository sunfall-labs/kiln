// kiln eval smoke test (authored for kiln, not from porffor).
const { Map, List, Set, fromJS, is } = require('immutable');

const m = Map({ a: 1, b: 2 });
console.log('get:', m.get('a'), 'size:', m.size);
const m2 = m.set('c', 3);
console.log('persistent:', m.size, m2.size, m2.get('c'));
console.log('merge:', JSON.stringify(m.merge({ b: 20, d: 4 }).toJS()));
console.log('delete:', m2.delete('a').size);

const list = List([1, 2, 3]);
console.log('list push:', JSON.stringify(list.push(4).toArray()));
console.log('list map:', JSON.stringify(list.map((x) => x * 2).toArray()));

const deep = fromJS({ x: { y: [1, 2, { z: 3 }] } });
console.log('getIn:', deep.getIn(['x', 'y', 2, 'z']));
console.log('setIn:', deep.setIn(['x', 'y', 0], 99).getIn(['x', 'y', 0]));

const s = Set([1, 2, 2, 3]);
console.log('set dedupe:', s.size, 'has:', s.has(2));
console.log('is equal:', is(Map({ a: 1 }), Map({ a: 1 })), is(List([1]), List([2])));
