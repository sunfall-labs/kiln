import {
  getProperty,
  setProperty,
  hasProperty,
  deleteProperty,
  escapePath
} from 'dot-prop';

console.log('--- Deep Get & Defaults ---');
const data = {
  user: {
    name: 'Alice',
    roles: ['admin', 'editor'],
    settings: {
      'site.domain': 'kiln.dev',
      active: false
    }
  }
};
console.log('get user.name:', getProperty(data, 'user.name'));
console.log('get array index user.roles[1]:', getProperty(data, 'user.roles[1]'));
console.log('get array dot index user.roles.0:', getProperty(data, 'user.roles.0'));
console.log('get boolean false user.settings.active:', getProperty(data, 'user.settings.active'));
console.log('get missing with default:', getProperty(data, 'user.missing.deep', 'fallback_val'));

console.log('--- Escaped paths for keys with dots ---');
const escapedKey = 'user.settings.' + escapePath('site.domain');
console.log('escaped path:', escapedKey);
console.log('get with escaped dot:', getProperty(data, escapedKey));

console.log('--- Deep Set & Intermediate creation ---');
setProperty(data, 'app.config.theme', 'nord');
setProperty(data, 'items[0].id', 101);
console.log('set app.config.theme:', getProperty(data, 'app.config.theme'));
console.log('set items[0].id:', getProperty(data, 'items[0].id'));

console.log('--- Has & Delete Property ---');
console.log('has user.name:', hasProperty(data, 'user.name'));
console.log('has user.unknown:', hasProperty(data, 'user.unknown'));
console.log('delete user.roles[0]:', deleteProperty(data, 'user.roles[0]'));
console.log('has user.roles[0] after delete:', hasProperty(data, 'user.roles[0]'));

console.log('--- Prototype Pollution Protection ---');
setProperty(data, '__proto__.polluted', 'yes');
setProperty(data, 'constructor.prototype.polluted', 'yes');
console.log('global polluted:', ({}).polluted === undefined);
