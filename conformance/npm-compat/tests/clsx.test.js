const clsx = require('clsx');

console.log('--- Strings, Numbers, Falsy ---');
console.log(JSON.stringify(clsx('btn', 'btn-lg')));
console.log(JSON.stringify(clsx('header', true, false, null, undefined, 0, 10, 'footer')));

console.log('--- Objects with varied value types ---');
console.log(JSON.stringify(clsx({
  active: true,
  disabled: false,
  visible: 1,
  hidden: 0,
  featured: 'yes',
  stale: null
})));

console.log('--- Nested Arrays and Objects ---');
console.log(JSON.stringify(clsx(
  'root',
  ['level-1', [false, 'level-2', [{ deep: true, omit: false }]]],
  { 'top-flag': true }
)));

console.log('--- Complex mixed conditionals ---');
const isSelected = true;
const isError = false;
const hasBadge = true;
console.log(JSON.stringify(clsx(
  'item',
  isSelected && 'item--selected',
  isError && 'item--error',
  [hasBadge && ['badge', 'badge--pill']]
)));

console.log('--- Empty or falsy-only inputs ---');
console.log(JSON.stringify(clsx()));
console.log(JSON.stringify(clsx('')));
console.log(JSON.stringify(clsx(null, undefined, false, 0)));
console.log(JSON.stringify(clsx({})));
