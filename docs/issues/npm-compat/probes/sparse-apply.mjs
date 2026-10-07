function substitute(string, number, suffix, future, locale) {
 console.log('args', string, number, suffix, future, locale);
 return locale[string].replace(/%d/i, number || 1);
}
const a = ['d']; a[2] = true; a[3] = true; a[4] = {d: 'a day'};
console.log('length', a.length, 1 in a);
console.log('result', substitute.apply(null, a));
const round = Math.round;
console.log('round', round(86400));
