const events = [];
const locales = new Proxy({ length: 2, 0: 'en', 1: 'de' }, {
  has(target, key) { events.push('has:' + key); return key in target; },
  get(target, key) { events.push('get:' + key); return target[key]; }
});
console.log(Intl.getCanonicalLocales(locales).join(','), events.join(','));
const functionLocale = () => {};
functionLocale.toString = () => 'en';
console.log(Intl.getCanonicalLocales([functionLocale]).join(','));
try {
  Intl.getCanonicalLocales([{ [Symbol.toPrimitive]() { return Symbol('locale'); } }]);
} catch (error) { console.log(error.name); }
let later = 0;
try {
  Intl.getCanonicalLocales({ length: Infinity, 0: 'bad_locale', get 1() { later++; return 'en'; } });
} catch (error) { console.log(error.name, later); }
console.log(Intl.getCanonicalLocales([, 'en']).join(','));
try { Intl.getCanonicalLocales([undefined]); }
catch (error) { console.log(error.name); }
