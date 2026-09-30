// kiln eval smoke test (authored for kiln, not from porffor).
import { atom, map, computed } from 'nanostores';

const $count = atom(0);
$count.set(5);
console.log($count.get());
const $doubled = computed($count, (c) => c * 2);
console.log($doubled.get());
const $user = map({ name: 'ada', age: 36 });
$user.setKey('age', 37);
console.log(JSON.stringify($user.get()));
const seen = [];
const unsub = $count.listen((v) => seen.push(v));
$count.set(6);
unsub();
$count.set(7);
console.log(seen.join(','), $count.get());
