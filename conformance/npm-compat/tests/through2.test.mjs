// kiln eval smoke test (authored for kiln, not from porffor).
import through2 from 'through2';

const out = [];
const t = through2(function (chunk, enc, cb) {
  this.push(String(chunk).toUpperCase());
  cb();
});
t.on('data', (c) => out.push(c.toString()));
await new Promise((resolve) => {
  t.on('end', resolve);
  t.write('hello');
  t.end(' world');
});
console.log(JSON.stringify(out));

const objs = [];
const t2 = through2.obj(function (chunk, enc, cb) {
  this.push(chunk * 2);
  cb();
});
t2.on('data', (c) => objs.push(c));
await new Promise((resolve) => {
  t2.on('end', resolve);
  t2.write(21);
  t2.end();
});
console.log(JSON.stringify(objs));
