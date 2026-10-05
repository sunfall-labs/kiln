import * as ns from './fixtures/module-namespace-slot-refresh.mjs';
import { update } from './fixtures/module-namespace-slot-refresh.mjs';

console.log(ns.value_000, ns.value_032, ns.value_063, Object.keys(ns).length);
update();
// Descriptors must observe live bindings before any Get can refresh a cached snapshot.
console.log(Object.getOwnPropertyDescriptor(ns, 'value_032').value,
  Object.getOwnPropertyDescriptors(ns).value_063.value,
  Reflect.getOwnPropertyDescriptor(ns, 'value_063').value);
// A namespace used as a prototype and an untrapped Proxy still resolve the same cells.
console.log(Object.create(ns).value_032,
  Object.getOwnPropertyDescriptor(new Proxy(ns, {}), 'value_063').value);
const computed = 'value_032';
console.log(Reflect.get(ns, computed));
Object.defineProperty(ns, 'value_032', { value: 500 });
console.log(ns.value_000, ns.value_032, ns.value_063);
const descriptor = Object.getOwnPropertyDescriptor(ns, 'value_063');
console.log(descriptor.value, descriptor.enumerable, descriptor.writable, descriptor.configurable);
