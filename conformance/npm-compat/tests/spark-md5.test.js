// kiln eval smoke test (authored for kiln, not from porffor).
const SparkMD5 = require('spark-md5');
console.log(SparkMD5.hash('hello'));
console.log(SparkMD5.hash(''));
const s = new SparkMD5();
s.append('hel');
s.append('lo');
console.log(s.end());
console.log(SparkMD5.hashBinary('hello').slice(0, 16));
console.log(SparkMD5.hash('hello') === s.end() ? 'same' : 'diff');
