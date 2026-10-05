const { produce, original } = require('immer');

console.log('--- Structural Sharing & Immutability ---');
const base = {
  user: { name: 'Alice', settings: { theme: 'dark' } },
  posts: [{ id: 1, title: 'First' }, { id: 2, title: 'Second' }],
  sidebar: { open: false },
  counter: 0
};

const next = produce(base, (draft) => {
  draft.posts.push({ id: 3, title: 'Third' });
  draft.user.settings.theme = 'light';
  draft.counter += 10;
  console.log('original counter:', original(draft).counter);
});

console.log('base !== next:', base !== next);
console.log('sidebar preserved:', base.sidebar === next.sidebar);
console.log('posts[0] preserved:', base.posts[0] === next.posts[0]);
console.log('user modified:', base.user !== next.user);
console.log('settings modified:', base.user.settings !== next.user.settings);
console.log('counter:', next.counter);

const frozen = produce({ a: { b: 2 } }, (d) => { d.a.b = 3; });
console.log(JSON.stringify(frozen));
console.log('frozen:', Object.isFrozen(frozen));

const unchanged = produce(base, () => {});
console.log('unchanged === base:', unchanged === base);

console.log('--- Replacing State ---');
const replaced = produce(base, () => ({ fresh: true }));
console.log(JSON.stringify(replaced));

console.log('--- Curried Producer ---');
const increment = produce((draft, delta) => {
  draft.counter += delta;
});
const curried = increment(next, 5);
console.log('curried counter:', curried.counter);
console.log('next counter intact:', next.counter);
