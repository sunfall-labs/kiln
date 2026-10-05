console.log('clock',Date.now(),performance.now(),typeof setImmediate,'setImmediate' in globalThis);
let n=0; const start=Date.now();
function tick(){ if(++n<1000)setImmediate(tick);else console.log('elapsed',Date.now()-start,performance.now()); }
setImmediate(tick);
