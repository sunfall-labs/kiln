function* g() { try { yield 1; yield 2 } finally { console.log('cleanup') } }
const it = g(); console.log(it.next().value);
console.log(JSON.stringify(it.return('early')));
