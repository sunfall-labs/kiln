// kiln eval smoke test (authored for kiln, not from porffor).
const {signal, computed, effect, batch} = require('@preact/signals-core');
const count = signal(1);
const doubled = computed(() => count.value * 2);
const seen = [];
effect(() => { seen.push(doubled.value); });
count.value = 5;
console.log(seen.join(','), doubled.value);
batch(() => { count.value = 10; count.value = 20; });
console.log(seen.join(','), count.value);
const other = signal('a');
console.log(other.value);
other.value = 'b';
console.log(other.value);
