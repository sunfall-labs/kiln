const promise = Promise.resolve(3);
const result = Object.assign(promise, { answer: 42 });
console.log(result === promise, promise.answer);
promise.then(value => console.log(value));
const date = Object.assign(new Date(0), { label: 'date' });
const copy = Object.assign({}, date);
console.log(date.getTime(), copy.label);
const map = new Map();
map.extra = 7;
console.log(Object.assign({}, map).extra);
const number = Object.assign(4, { extra: 9 });
console.log(typeof number, number.valueOf(), number.extra);
const text = Object.assign('ab', { extra: 2 });
console.log(text.valueOf(), text.extra);
try { Object.assign('ab', { 0: 'x' }); }
catch (error) { console.log(error instanceof TypeError); }
try { Object.assign(null, {}); }
catch (error) { console.log(error instanceof TypeError); }
