// kiln eval smoke test (authored for kiln, not from porffor).
import {diffLines, diffWords, diffJson} from 'diff';
const parts = diffLines('a\nb\n', 'a\nc\n');
console.log(parts.map((p) => (p.added ? '+' : p.removed ? '-' : '=') + p.value.trim()).join('|'));
console.log(
  diffWords('hello world', 'hello there')
    .filter((p) => p.added || p.removed)
    .map((p) => (p.added ? '+' : '-') + p.value)
    .join(''),
);
console.log(diffJson({a: 1}, {a: 2}).length > 0);
console.log(diffLines('same', 'same').every((p) => !p.added && !p.removed));
