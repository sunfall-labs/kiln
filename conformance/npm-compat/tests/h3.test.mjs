// kiln eval smoke test (authored for kiln, not from porffor).
import { createApp, eventHandler, toNodeListener, getQuery, readBody } from 'h3';

const app = createApp();
app.use('/hello', eventHandler(() => 'hello kiln'));
app.use('/json', eventHandler(() => ({ ok: true, n: 42 })));
app.use('/greet', eventHandler((event) => {
  const q = getQuery(event);
  return 'hi ' + (q.name || 'anon');
}));
app.use('/echo', eventHandler(async (event) => {
  const body = await readBody(event);
  return { youSent: body };
}));

const r1 = await app.fetch(new Request('http://x/hello'));
console.log('text route:', r1.status, await r1.text());
const r2 = await app.fetch(new Request('http://x/json'));
console.log('json route:', r2.status, JSON.stringify(await r2.json()));
const r3 = await app.fetch(new Request('http://x/greet?name=kiln'));
console.log('query route:', await r3.text());
const r4 = await app.fetch(new Request('http://x/echo', {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify({ a: 1 })
}));
console.log('body route:', JSON.stringify(await r4.json()));
console.log('toNodeListener is fn:', typeof toNodeListener === 'function');
