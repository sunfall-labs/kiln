function assert(x) { if (!x) throw Error('assertion'); }
(function() {
  'use strict';
  let a = []; a[-0] = 12; a[1] = 34; a[4] = 56;
  assert(a.length === 5 && a[0] === 12 && a[1] === 34 && a[4] === 56);
  assert(!(2 in a) && !(3 in a));
  a.length = 0; a[0] = 7; assert(a.length === 1 && a[0] === 7);
  let locked = []; Object.preventExtensions(locked);
  let caught = false; try { locked[0] = 1; } catch (e) { caught = e instanceof TypeError; }
  assert(caught);
  let frozen = Object.freeze([1]); caught = false;
  try { frozen[0] = 2; } catch (e) { caught = e instanceof TypeError; }
  assert(caught && frozen[0] === 1);
  let exotic = []; let set = 0;
  Object.defineProperty(exotic, '0', {set(v) {set = v}, configurable: true});
  exotic[0] = 17; assert(set === 17);
  let proto = {}; Object.defineProperty(proto, '0', {value: 1, writable: false});
  let child = []; Object.setPrototypeOf(child, proto); caught = false;
  try { child[0] = 2; } catch (e) { caught = e instanceof TypeError; }
  assert(caught && child[0] === 1);
  let keyed = []; let calls = 0;
  keyed[{toString() { calls++; return '0'; }}] = 3;
  assert(calls === 1 && keyed[0] === 3);
  let names = []; names[4294967295] = 4; names[1.5] = 5;
  assert(names.length === 0 && names[4294967295] === 4 && names[1.5] === 5);
})();
console.log('write guards passed');
