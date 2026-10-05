const ini = require('ini');

console.log('--- Nested sections and arrays ---');
const iniText = `
# Global properties
app_name = Kiln Compiler
version = 1.0.0
debug = true

; Database configuration
[database]
host = 127.0.0.1
port = 5432
enabled = true

[database.pool]
min = 2
max = 10

[features]
tags[] = speed
tags[] = memory
tags[] = native
`;

const parsed = ini.parse(iniText);
console.log('app_name:', parsed.app_name);
console.log('debug type:', typeof parsed.debug, parsed.debug);
console.log('database.host:', parsed.database.host);
console.log('database.port type:', typeof parsed.database.port, parsed.database.port);
console.log('database.pool.max:', parsed.database.pool.max);
console.log('features.tags:', JSON.stringify(parsed.features.tags));

console.log('--- Stringify with options ---');
const data = {
  service: {
    name: 'worker',
    active: true,
    replicas: 3
  }
};
console.log(ini.stringify(data, { whitespace: true }).trim());

console.log('--- Safe roundtrip ---');
const roundtrip = ini.parse(ini.stringify(parsed));
console.log('roundtrip database.port:', roundtrip.database.port);
console.log('roundtrip tags length:', roundtrip.features.tags.length);
