// kiln eval smoke test (authored for kiln, not from porffor).
require('regenerator-runtime/runtime');

console.log('runtime installed:', typeof regeneratorRuntime === 'object');
console.log('has mark:', typeof regeneratorRuntime.mark === 'function');
console.log('has wrap:', typeof regeneratorRuntime.wrap === 'function');
console.log('has async:', typeof regeneratorRuntime.async === 'function');
console.log('has awrap:', typeof regeneratorRuntime.awrap === 'function');

const genFn = regeneratorRuntime.mark(function counter() {
  return regeneratorRuntime.wrap(function counter$(_ctx) {
    while (1) switch (_ctx.prev = _ctx.next) {
      case 0: _ctx.next = 2; return 10;
      case 2: _ctx.next = 4; return 20;
      case 4: case 'end': return _ctx.stop();
    }
  }, counter);
});
const out = [];
for (const v of genFn()) out.push(v);
console.log('hand-rolled generator:', JSON.stringify(out));

const aw = regeneratorRuntime.awrap(7);
console.log('awrap shape:', aw && aw.__await === 7);
