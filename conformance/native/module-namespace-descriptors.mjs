import * as ns from './fixtures/module-namespace-descriptors.mjs';
console.log(Reflect.defineProperty(ns, 'value', {value: 2}));
ns.update();
console.log(Reflect.defineProperty(ns, 'value', {value: 2}), ns.value);
console.log(Reflect.defineProperty(ns, 'nan', {value: NaN}));
console.log(Reflect.defineProperty(ns, 'zero', {value: 0}));
for (const descriptor of [{writable: false}, {enumerable: false}, {configurable: true}, {get: undefined}]) {
  console.log(Reflect.defineProperty(ns, 'value', descriptor));
}
console.log(Reflect.defineProperty(ns, 'missing', {}));
