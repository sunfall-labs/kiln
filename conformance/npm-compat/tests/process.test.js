// kiln eval smoke test (authored for kiln, not from porffor).
// NOTE: required by relative path because bare 'process' resolves to the Node builtin.
const proc = require('../node_modules/process');

console.log(typeof proc.nextTick);
console.log(proc.platform === process.platform);
console.log(typeof proc.env);
console.log(typeof proc.cwd);

let ran = false;
proc.nextTick(() => {
  ran = true;
  console.log('nextTick ran');
});
console.log('scheduled', ran === false);
