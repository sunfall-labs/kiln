// kiln eval smoke test (authored for kiln, not from porffor).
const ini = require('ini');
console.log(JSON.stringify(ini.parse('[section]\nkey=value\nnum=42')));
console.log(JSON.stringify(ini.parse(';comment\n[a]\nb=1\n[c]\nd=2')));
console.log(ini.stringify({ section: { key: 'value', num: 42 } }));
console.log(JSON.stringify(ini.parse('top=yes\n[sec]\nx=1')));
