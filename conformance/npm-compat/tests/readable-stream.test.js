// kiln eval smoke test (authored for kiln, not from porffor).
const { Readable, Writable, Transform, pipeline } = require('readable-stream');

const collected = [];
const w = new Writable({
  write(chunk, enc, cb) {
    collected.push(chunk.toString());
    cb();
  },
});
w.write('hello');
w.end(' world');
w.on('finish', () => console.log(collected.join('')));

(async () => {
  const out = [];
  for await (const x of Readable.from(['a', 'b', 'c'])) out.push(x);
  console.log(JSON.stringify(out));

  const upper = new Transform({
    transform(chunk, enc, cb) {
      cb(null, chunk.toString().toUpperCase());
    },
  });
  const res = [];
  await new Promise((resolve, reject) => {
    pipeline(Readable.from(['x', 'y']), upper, (err) => (err ? reject(err) : resolve()));
    upper.on('data', (c) => res.push(c.toString()));
  });
  console.log(JSON.stringify(res));
})();
