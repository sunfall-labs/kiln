import {
  diffChars,
  diffWords,
  diffLines,
  diffJson,
  diffArrays,
  createPatch,
  applyPatch,
  structuredPatch
} from 'diff';

console.log('--- diffChars ---');
const charDiff = diffChars('antigravity', 'anti-gravitas');
console.log(charDiff.map(p => (p.added ? '+' : p.removed ? '-' : '=') + p.value).join('|'));

console.log('--- diffWords ---');
const wordDiff = diffWords('hello world from kiln', 'hello brave new world from kiln');
console.log(wordDiff.filter(p => p.added || p.removed).map(p => (p.added ? '+' : '-') + p.value).join(''));

console.log('--- diffLines ---');
const lineDiff = diffLines('a\nb\n', 'a\nc\n');
console.log(lineDiff.map(p => (p.added ? '+' : p.removed ? '-' : '=') + p.value.trim()).join('|'));
console.log('identical lines:', diffLines('same', 'same').every(p => !p.added && !p.removed));

console.log('--- diffArrays ---');
const arrDiff = diffArrays([1, 2, 3, 5], [1, 2, 4, 5, 6]);
console.log(arrDiff.map(p => (p.added ? '+' : p.removed ? '-' : '=') + p.value.join(',')).join('|'));

console.log('--- diffJson ---');
const jsonDiff = diffJson(
  { name: 'kiln', version: 1, tags: ['fast'] },
  { name: 'kiln', version: 2, tags: ['fast', 'native'] }
);
console.log(jsonDiff.filter(p => p.added || p.removed).map(p => (p.added ? '+' : '-') + p.value.trim()).join('; '));

console.log('--- createPatch and applyPatch ---');
const originalText = 'line1\nline2\nline3\n';
const modifiedText = 'line1\nline2-modified\nline3\nline4\n';
const patch = createPatch('sample.txt', originalText, modifiedText, 'header1', 'header2');
console.log(patch.split('\n').slice(0, 4).join('\n'));

const patched = applyPatch(originalText, patch);
console.log('patch applied successfully:', patched === modifiedText);

console.log('--- structuredPatch ---');
const sp = structuredPatch('a.txt', 'b.txt', originalText, modifiedText);
console.log('hunks count:', sp.hunks.length);
console.log('hunk 0 lines:', sp.hunks[0].lines.length);
