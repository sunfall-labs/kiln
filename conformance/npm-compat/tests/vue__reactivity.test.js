// kiln eval smoke test (authored for kiln, not from porffor).
const {ref, reactive, computed, effect} = require('@vue/reactivity');
const cnt = ref(0);
const dbl = computed(() => cnt.value * 2);
const log = [];
effect(() => { log.push(cnt.value); });
cnt.value = 5;
console.log(log.join(','), dbl.value);
const st = reactive({items: [1]});
st.items.push(2);
console.log(st.items.length, st.items.join(','));
const r = ref({n: 1});
console.log(r.value.n);
