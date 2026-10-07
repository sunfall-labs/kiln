// Computed keys on arrays use the same ordinary-property coercion as on objects once the key
// is not an array index. Array fast paths must not reject these valid property names.
const array = [];
for (const key of [undefined, null, true, 1.5]) {
  array[key] = String(key);
  console.log(array[key]);
}
console.log(Object.keys(array).join(','));
