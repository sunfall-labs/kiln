// kiln eval smoke test (authored for kiln, not from porffor).
const Stream = require('stream-browserify');

const collected = [];
const w = new Stream.Writable({
  write(chunk, enc, cb) {
    collected.push(chunk.toString());
    cb();
  },
});
w.write('ab');
w.end('cd');
w.on('finish', () => console.log(collected.join('')));

(async () => {
  const out = [];
  for await (const x of Stream.Readable.from(['m', 'n'])) out.push(x);
  console.log(JSON.stringify(out));

  const t = new Stream.Transform({
    transform(chunk, enc, cb) {
      cb(null, String(chunk).toUpperCase());
    },
  });
  let got = '';
  t.on('data', (c) => {
    got += c.toString();
  });
  t.on('end', () => console.log(got));
  t.write('q');
  t.end('z');
})();
