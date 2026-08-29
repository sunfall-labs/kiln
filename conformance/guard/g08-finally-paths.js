function a() { try { return 'A' } finally { console.log('finA') } } console.log(a());
function b() { try { throw new Error('B') } finally { console.log('finB') } }
try { b() } catch (e) { console.log('caught', e.message) }
function c() { for (let i = 0; i < 3; i++) { try { if (i === 1) continue; if (i === 2) break } finally { console.log('fin', i) } } return 'done' }
console.log(c());
