import http from 'node:http';

async function check(mode) {
  const server = http.createServer((request, response) => {
    response.writeHead(200, { 'content-type': 'text/plain' });
    response.write('first');
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  const controller = new AbortController();
  const response = await fetch('http://127.0.0.1:' + server.address().port,
    { signal: controller.signal });
  const reader = response.body.getReader();
  const first = await reader.read();
  console.log(mode, response.status, new TextDecoder().decode(first.value), first.done);
  if (mode === 'abort') {
    const pending = reader.read();
    controller.abort();
    try {
      await pending;
      console.log('unexpected read');
    } catch (error) {
      console.log('read rejected', error.name);
    }
  } else {
    await reader.cancel();
    console.log('reader cancelled');
  }
  await new Promise(resolve => server.close(resolve));
  console.log('server closed');
}
await check('abort');
await check('cancel');

const server = http.createServer((request, response) => response.end('done'));
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
const controller = new AbortController();
const pending = fetch('http://127.0.0.1:' + server.address().port, { signal: controller.signal });
controller.abort();
try {
  await pending;
  console.log('unexpected pending response');
} catch (error) {
  console.log('pending rejected', error.name);
}
await new Promise(resolve => server.close(resolve));
console.log('pending server closed');
