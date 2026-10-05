import * as ns from './fixtures/module-namespace-slot-refresh.mjs';
import { update } from './fixtures/module-namespace-slot-refresh.mjs';

console.log(ns.value_000, ns.value_032, ns.value_063, Object.keys(ns).length);
update();
console.log(ns.value_000, ns.value_032, ns.value_063);
const descriptor = Object.getOwnPropertyDescriptor(ns, 'value_063');
console.log(descriptor.value, descriptor.enumerable, descriptor.writable, descriptor.configurable);
