console.log('a');
const t = setTimeout(() => console.log('never'), 5); clearTimeout(t);
setTimeout((x) => console.log('args', x), 1, 'p');
setTimeout(() => console.log('late'), 20);
let n = 0; const iv = setInterval(() => { n++; if (n === 2) clearInterval(iv); console.log('iv', n) }, 2);
queueMicrotask(() => console.log('micro'));
console.log('b');
