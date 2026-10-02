const events = [];
const locales = new Proxy({ length: 2, 0: 'en', 1: 'de' }, {
  has(target, key) { events.push('has:' + key); return key in target; },
  get(target, key) { events.push('get:' + key); return target[key]; }
});
console.log("I".toLocaleLowerCase(locales), events.join(','));
const functionLocale = () => {};
functionLocale.toString = () => 'en';
console.log("I".toLocaleLowerCase([functionLocale]));
try {
  "I".toLocaleLowerCase([{ [Symbol.toPrimitive]() { return Symbol('locale'); } }]);
} catch (error) { console.log(error.name); }
let later = 0;
try {
  "I".toLocaleLowerCase({ length: Infinity, 0: 'bad_locale', get 1() { later++; return 'en'; } });
} catch (error) { console.log(error.name, later); }
console.log("I".toLocaleLowerCase([, 'en']));
try { "I".toLocaleLowerCase([undefined]); }
catch (error) { console.log(error.name); }
