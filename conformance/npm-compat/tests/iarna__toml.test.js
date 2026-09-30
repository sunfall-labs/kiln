// kiln eval smoke test (authored for kiln, not from porffor).
const TOML = require('@iarna/toml');
console.log(JSON.stringify(TOML.parse('title = "x"\n[n]\na = 1\n')));
console.log(JSON.stringify(TOML.stringify({b: 2})));
console.log(JSON.stringify(TOML.parse('d = 2024-01-02T03:04:05Z')));
console.log(TOML.stringify({arr: [1, 2]}).trim());
console.log(JSON.stringify(TOML.parse('t = true\nf = 1.5\n')));
