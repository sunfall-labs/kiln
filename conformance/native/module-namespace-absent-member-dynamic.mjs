// The companion of `module-namespace-absent-member.mjs` for namespaces the native backend plans
// completely: dynamic import() namespaces, `in`, computed keys and reflection. An absent name is
// `undefined` to [[Get]], false to [[HasProperty]], and has no own property descriptor.
// Dynamic import() namespaces, with `in` and computed keys.
const key = 'missing';
const dyn = await import('./fixtures/module-namespace-absent-member-dynamic.mjs');
console.log(dyn.c, dyn.missing, typeof dyn.missing, 'missing' in dyn, 'c' in dyn);
console.log(dyn[key], key in dyn, Reflect.has(dyn, key), Object.keys(dyn).join(','));
console.log(Object.getOwnPropertyDescriptor(dyn, key));
const { missing: dynMissing = 'dyn-default' } = dyn;
console.log(dynMissing);
const dynBarrel = await import('./fixtures/module-namespace-absent-member-dynamic-barrel.mjs');
console.log(dynBarrel.d, dynBarrel.missing, 'missing' in dynBarrel, typeof dynBarrel.missing);
