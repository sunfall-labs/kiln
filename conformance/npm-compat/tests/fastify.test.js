// kiln eval smoke test (authored for kiln, not from porffor).
const http = require('node:http');
const legacyResponse = Object.create(http.ServerResponse.prototype);
http.ServerResponse.call(legacyResponse, { method: 'GET' });
legacyResponse.setHeader('X-Legacy-Probe', 'ok');
console.log('legacy response:', legacyResponse.req.method, legacyResponse.getHeader('x-legacy-probe'),
  legacyResponse instanceof http.ServerResponse);

const fastify = require('fastify')();
fastify.get('/hello', async () => ({ msg: 'hi' }));
fastify.get('/add/:a/:b', async req => ({ sum: Number(req.params.a) + Number(req.params.b) }));
fastify.post('/echo', async req => ({ youSent: req.body }));
(async () => {
  const r1 = await fastify.inject({ method: 'GET', url: '/hello' });
  console.log(r1.statusCode, r1.body);
  const r2 = await fastify.inject({ method: 'GET', url: '/add/3/4' });
  console.log(r2.statusCode, r2.body);
  const r3 = await fastify.inject({ method: 'POST', url: '/echo', payload: { n: 7 } });
  console.log(r3.statusCode, r3.body);
  const r4 = await fastify.inject({ method: 'GET', url: '/nope' });
  console.log(r4.statusCode);
  await fastify.close();
})();
