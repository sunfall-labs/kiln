// kiln eval smoke test (authored for kiln, not from porffor).
const { match, compile, pathToRegexp } = require('path-to-regexp');

const m1 = match('/user/:id')('/user/42');
console.log(JSON.stringify(m1.params));

const m2 = match('/user/:id')('/post/42');
console.log(m2 === false);

const m3 = match('/files/:file.:ext')('/files/report.pdf');
console.log(JSON.stringify(m3.params));

const toPath = compile('/user/:id');
console.log(toPath({ id: '7' }));

const { regexp, keys } = pathToRegexp('/order/:id');
console.log(regexp.test('/order/123'), regexp.test('/order/'));
console.log(keys[0].name);
