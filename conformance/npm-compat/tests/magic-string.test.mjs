import MagicString, { Bundle } from 'magic-string';

console.log('--- In-place string editing ---');
const s = new MagicString('export const answer = 42;');
s.prepend('// header\n');
s.append('\n// footer');
s.overwrite(13, 19, 'solution');
s.appendRight(22, ' /* the value */');
console.log(s.toString());
console.log('hasChanged:', s.hasChanged(), new MagicString('same').hasChanged());

console.log('--- Snip & Slice ---');
const code = new MagicString('function test() { const x = 1; return x; }');
console.log('slice body:', code.slice(16, 41));
const snippet = code.snip(18, 30);
console.log('snip instance:', snippet.toString());

console.log('--- Indentation & Trimming ---');
const multi = new MagicString('line 1\nline 2\nline 3\n');
multi.indent('  ');
console.log(multi.toString());

console.log('--- Sourcemap Generation ---');
const map = s.generateMap({
  source: 'input.js',
  file: 'output.js',
  includeContent: true
});
console.log('map version:', map.version);
console.log('map file:', map.file);
console.log('map sources:', map.sources);
console.log('has mappings:', typeof map.mappings === 'string' && map.mappings.length > 0);

console.log('--- Decoded Map ---');
const decoded = s.generateDecodedMap({ source: 'input.js' });
console.log('decoded mappings is array:', Array.isArray(decoded.mappings));
console.log('decoded lines count:', decoded.mappings.length);

console.log('--- MagicString Bundle ---');
const bundle = new Bundle();
bundle.addSource({
  filename: 'foo.js',
  content: new MagicString('const a = 1;')
});
bundle.addSource({
  filename: 'bar.js',
  content: new MagicString('const b = 2;')
});
console.log('bundle output:');
console.log(bundle.toString().trim());
