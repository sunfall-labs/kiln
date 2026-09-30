// kiln eval smoke test (authored for kiln, not from porffor).
const { AutoRouter } = require('itty-router');
const router = AutoRouter();
router.get('/hello/:name', (req) => new Response('hi ' + req.params.name));
router.post('/sum', async (req) => { const body = await req.json(); return Response.json({ sum: body.a + body.b }); });
(async () => {
  const r1 = await router.fetch(new Request('http://x/hello/ada'));
  console.log(r1.status, await r1.text());
  const r2 = await router.fetch(new Request('http://x/sum', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ a: 2, b: 3 }) }));
  console.log(r2.status, JSON.stringify(await r2.json()));
  console.log('routes:', router.routes.length);
  const r3 = await router.fetch(new Request('http://x/missing'));
  console.log(r3.status);
})();
