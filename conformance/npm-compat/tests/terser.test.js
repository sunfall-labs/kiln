// kiln eval smoke test (authored for kiln, not from porffor).
const { minify } = require('terser');

(async () => {
  const r1 = await minify('function add(a, b) { return a + b; } console.log(add(1, 2));');
  console.log('basic:', r1.code);

  const r2 = await minify('const longVariableName = 40 + 2; console.log(longVariableName);', { mangle: { toplevel: true } });
  console.log('mangle:', r2.code);

  const r3 = await minify('if (true) { console.log("dead"); } else { console.log("gone"); }', { compress: { dead_code: true, conditionals: true } });
  console.log('compress:', r3.code);

  const r4 = await minify('var x = 1; var y = 2; console.log(x + y);', { format: { beautify: true } });
  console.log('beautify has newline:', r4.code.includes('\n'));

  let parseErr = '';
  try { await minify('const x: number = 1;', {}); } catch (e) { parseErr = String(e.message).slice(0, 40); }
  console.log('invalid js rejects:', parseErr.length > 0, JSON.stringify(parseErr));

  const r6 = await minify({ 'a.js': 'var a = 1;', 'b.js': 'var b = 2;' });
  console.log('multi-file:', r6.code);
})().catch((e) => { console.error('FAILED', e); process.exit(1); });
