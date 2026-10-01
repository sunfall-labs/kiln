// kiln eval smoke test (authored for kiln, not from porffor).
import boxen from 'boxen';

console.log(boxen('hello kiln', { padding: 1 }));
console.log('---');
console.log(boxen('title test', { title: 'Box', titleAlignment: 'center', borderStyle: 'double' }));
console.log('---');
console.log(boxen('multi\nline\ntext', { padding: { top: 0, bottom: 0, left: 2, right: 2 }, margin: 0 }));
console.log('---');
console.log(boxen('round', { borderStyle: 'round', borderColor: 'red' }));
