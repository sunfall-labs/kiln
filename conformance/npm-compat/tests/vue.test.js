// kiln eval smoke test (authored for kiln, not from porffor).
const { ref, computed, reactive, isRef, isReactive, version } = require('vue');

const count = ref(1);
console.log(isRef(count), count.value);
count.value = 2;
console.log(count.value);

const doubled = computed(() => count.value * 2);
console.log(doubled.value);
count.value = 5;
console.log(doubled.value);

const state = reactive({ nested: { n: 1 } });
console.log(isReactive(state), isReactive(state.nested));
state.nested.n = 42;
console.log(state.nested.n);

console.log(typeof version === 'string' && version.startsWith('3.'));
