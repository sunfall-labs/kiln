// kiln eval smoke test (authored for kiln, not from porffor).
const async = require('async');
async.map([1, 2, 3], (x, cb) => cb(null, x * 2), (err, res) => console.log(JSON.stringify(res)));
async.series([(cb) => cb(null, 'a'), (cb) => cb(null, 'b')], (err, res) => console.log(JSON.stringify(res)));
async.filter([1, 2, 3, 4], (x, cb) => cb(null, x % 2 === 0), (err, res) => console.log(JSON.stringify(res)));
async.times(3, (n, cb) => cb(null, n * 10), (err, res) => console.log(JSON.stringify(res)));
async.eachOf({a: 1, b: 2}, (v, k, cb) => cb(null), (err) => console.log('eachOf:' + (err ? 'err' : 'ok')));
