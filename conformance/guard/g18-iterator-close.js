const log = [];
const it = { [Symbol.iterator]() { return { next: () => ({ value: 1, done: false }), return() { log.push('closed'); return { done: true } } } } };
for (const x of it) break;
console.log(log.join(',') || 'not closed');
