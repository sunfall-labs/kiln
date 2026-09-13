const events = [];
const specifier = {
  toString() {
    events.push('specifier');
    return './r14-dynamic-import-target.mjs';
  },
};
const options = {
  get with() {
    events.push('with');
    return {
      get type() {
        events.push('type');
        return 'json';
      },
    };
  },
};

const loaded = import(specifier, options);
console.log('returned promise');
try {
  await loaded;
} catch (error) {
  console.log(error.name, error.message);
}
console.log(events.join(','));

try {
  const throwing = import('./r14-dynamic-import-target.mjs', {
    get with() {
      throw new Error('getter boom');
    },
  });
  console.log('throwing returned promise');
  await throwing;
} catch (error) {
  console.log('getter rejection', error.message);
}

const withoutOptions = await import('./r14-dynamic-import-target.mjs');
const emptyOptions = await import('./r14-dynamic-import-target.mjs', { with: {} });
console.log(withoutOptions.default, emptyOptions.default);
