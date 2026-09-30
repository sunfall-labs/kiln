// kiln eval smoke test (authored for kiln, not from porffor).
import TOML from 'smol-toml';

console.log(JSON.stringify(TOML.parse('a = 1\nb = "x"')));
console.log(TOML.stringify({ a: 1, b: 'x', c: [1, 2] }).trim());

const d = TOML.parse('d = 1979-05-27T07:32:00Z');
console.log(d.d instanceof Date);

try {
  TOML.parse('a = ');
  console.log('no-throw');
} catch (e) {
  console.log(e instanceof Error, e.name);
}

console.log(JSON.stringify(TOML.parse('[t]\nx.y = 1')));
