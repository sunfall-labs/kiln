// kiln eval smoke test (authored for kiln, not from porffor).
const PubSub = require('pubsub-js');

const out = [];
const token = PubSub.subscribe('topic.a', (msg, data) => out.push([msg, data]));

PubSub.publishSync('topic.a', { n: 1 });
PubSub.publishSync('topic.a', [1, 2]);
console.log(JSON.stringify(out));
console.log(typeof token);

PubSub.unsubscribe(token);
PubSub.publishSync('topic.a', { n: 2 });
console.log(JSON.stringify(out));

let count = 0;
PubSub.subscribe('topic.b', () => { count += 1; });
PubSub.publishSync('topic.b', null);
PubSub.publishSync('topic.b', null);
console.log(count);
console.log(PubSub.countSubscriptions('topic.b'));
