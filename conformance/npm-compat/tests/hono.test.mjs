// kiln eval smoke test (authored for kiln, not from porffor).
import { Hono } from 'hono';

const app = new Hono();
app.get('/hello/:name', (c) => c.text('hi ' + c.req.param('name')));
app.post('/echo', async (c) => c.json(await c.req.json()));
app.get('/status', (c) => c.json({ ok: true }, 201));

const r1 = await app.request('/hello/ada');
console.log(r1.status, await r1.text());
const r2 = await app.request('/echo', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ a: 1 }) });
console.log(r2.status, JSON.stringify(await r2.json()));
const r3 = await app.request('/status');
console.log(r3.status, JSON.stringify(await r3.json()));
const r4 = await app.request('/missing');
console.log(r4.status);
