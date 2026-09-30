// kiln eval smoke test (authored for kiln, not from porffor).
const _extends = require('@babel/runtime/helpers/extends');
const _interopRequireDefault = require('@babel/runtime/helpers/interopRequireDefault');
const _typeof = require('@babel/runtime/helpers/typeof');
console.log(JSON.stringify(_extends({a: 1}, {b: 2}, {a: 9})));
console.log(JSON.stringify(_interopRequireDefault({__esModule: true, default: 5})));
console.log(JSON.stringify(_interopRequireDefault({x: 1})));
console.log(_typeof('s'), _typeof(Symbol('t')));
