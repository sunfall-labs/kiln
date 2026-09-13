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

async function verdict(label, promise) {
  try {
    await promise;
    console.log(label, 'ok');
  } catch (error) {
    console.log(label, error.name);
  }
}

await verdict('options-null', import('./r14-dynamic-import-target.mjs', null));
await verdict('options-number', import('./r14-dynamic-import-target.mjs', 1));
await verdict('with-null', import('./r14-dynamic-import-target.mjs', { with: null }));
await verdict('with-number', import('./r14-dynamic-import-target.mjs', { with: 1 }));
await verdict('type-number', import('./r14-dynamic-import-target.mjs', { with: { type: 123 } }));
await verdict('type-undefined', import('./r14-dynamic-import-target.mjs', { with: { type: undefined } }));
await verdict('type-unsupported', import('./r14-dynamic-import-target.mjs', { with: { type: 'json' } }));
await verdict('unknown-attribute', import('./r14-dynamic-import-target.mjs', { with: { mode: 'file' } }));
await verdict('unknown-before-throw', import('./r14-dynamic-import-target.mjs', {
  with: { mode: 'file', get type() { throw new Error('late getter'); } },
}));
await verdict('bad-before-throw', import('./r14-dynamic-import-target.mjs', {
  with: { type: 'json', get later() { throw new Error('later getter'); } },
}));
await verdict('asset-missing-type', import('./r14-dynamic-import-asset.wasm'));
await verdict('source-file-type', import('./r14-dynamic-import-target.mjs', { with: { type: 'file' } }));
await verdict('throwing-attribute', import('./r14-dynamic-import-target.mjs', {
  with: { get type() { throw new Error('type boom'); } },
}));
const inherited = Object.create({ type: 'file' });
await verdict('inherited-type', import('./r14-dynamic-import-target.mjs', { with: inherited }));
const nonEnumerable = {};
Object.defineProperty(nonEnumerable, 'type', { value: 'file', enumerable: false });
await verdict('non-enumerable-type', import('./r14-dynamic-import-target.mjs', { with: nonEnumerable }));
