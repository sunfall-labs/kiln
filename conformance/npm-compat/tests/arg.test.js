// kiln eval smoke test (authored for kiln, not from porffor).
const arg = require('arg');

const parsed = arg(
  { '--verbose': Boolean, '--name': String, '--count': Number, '-v': '--verbose' },
  { argv: ['--verbose', '--name', 'kiln', '--count', '42', 'extra'] }
);
console.log('verbose:', parsed['--verbose']);
console.log('name:', parsed['--name']);
console.log('count:', parsed['--count'], typeof parsed['--count']);
console.log('rest:', JSON.stringify(parsed._));

const alias = arg({ '--verbose': Boolean, '-v': '--verbose' }, { argv: ['-v'] });
console.log('alias -v:', alias['--verbose']);

try {
  arg({ '--known': Boolean }, { argv: ['--bogus'] });
  console.log('no-throw');
} catch (e) {
  console.log('unknown-option code:', e.code);
}

const permissive = arg({ '--known': Boolean }, { argv: ['--bogus', '--known'], permissive: true });
console.log('permissive known:', permissive['--known'], 'rest:', JSON.stringify(permissive._));
