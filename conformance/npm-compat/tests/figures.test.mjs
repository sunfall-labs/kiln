// kiln eval smoke test (authored for kiln, not from porffor).
import figures from 'figures';

console.log('tick:', JSON.stringify(figures.tick));
console.log('cross:', JSON.stringify(figures.cross));
console.log('pointer:', JSON.stringify(figures.pointer));
console.log('bullet:', JSON.stringify(figures.bullet));
console.log('star:', JSON.stringify(figures.star));
console.log('warning:', JSON.stringify(figures.warning));
console.log('is object with keys:', typeof figures === 'object' && Object.keys(figures).length > 10);
