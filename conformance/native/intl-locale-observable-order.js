function locale(locales) { return new Intl.DateTimeFormat(locales).resolvedOptions().locale; }
const events = [];
const locales = new Proxy({ length: 2, 0: 'en', 1: 'de' }, {
  has(target, key) { events.push('has:' + key); return key in target; },
  get(target, key) { events.push('get:' + key); return target[key]; }
});
console.log(locale(locales), events.join(','));
const functionLocale = () => {};
functionLocale.toString = () => 'en';
console.log(locale([functionLocale]));
try {
  locale([{ [Symbol.toPrimitive]() { return Symbol('locale'); } }]);
} catch (error) { console.log(error.name); }
let later = 0;
try {
  locale({ length: Infinity, 0: 'bad_locale', get 1() { later++; return 'en'; } });
} catch (error) { console.log(error.name, later); }
console.log(locale([, 'en']));
try { locale([undefined]); }
catch (error) { console.log(error.name); }

const symbolLocale = Symbol('locales');
const symbolEvents = [];
for (const key of ['length', '0']) {
  Object.defineProperty(Symbol.prototype, key, {
    configurable: true,
    get() {
      if (this === symbolLocale || !(this instanceof Symbol) || this.valueOf() !== symbolLocale) {
        throw new Error(`ToObject receiver lost for ${key}`);
      }
      symbolEvents.push(key);
      return key === 'length' ? 1 : 'en';
    }
  });
}
console.log(locale(symbolLocale), symbolEvents.join(','));
delete Symbol.prototype.length;
delete Symbol.prototype[0];

// Only Intl.Locale is a single-element special case. Other Intl objects are array-like inputs;
// without a length they request the default locale list.
console.log(locale(new Intl.DateTimeFormat()));
