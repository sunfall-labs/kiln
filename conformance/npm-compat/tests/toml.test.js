// kiln eval smoke test (authored for kiln, not from porffor).
const toml = require('toml');

const parsed = toml.parse(`
title = "demo"
[owner]
name = "Ann"
[deps]
list = [1, 2, 3]
pi = 3.14
`);
console.log(parsed.title);
console.log(parsed.owner.name);
console.log(JSON.stringify(parsed.deps.list), parsed.deps.pi);

try {
  toml.parse('broken = = =');
  console.log('no-throw');
} catch (e) {
  console.log(e.name);
}
console.log(toml.parse('d = 1979-05-27T07:32:00Z').d.toISOString());
