const marker = Symbol('marker');
function callable() {}
callable.extra = 7;
callable[marker] = 8;
const data = Object.getOwnPropertyDescriptors(callable);
console.log(data.extra.value, data[marker].value, data.name.enumerable, data.length.writable);
console.log(Object.getOwnPropertyDescriptors('ab')['1'].value);
console.log(Object.keys(Object).length, Object.keys(7).length);
for (const value of [null, undefined]) {
  try { Object.getOwnPropertyDescriptors(value); }
  catch (error) { console.log(error instanceof TypeError); }
}
const array = [4, , 6];
array.extra = 9;
const rows = Object.getOwnPropertyDescriptors(array);
console.log(rows['0'].value, rows['1'] === undefined, rows.length.value, rows.extra.value);
