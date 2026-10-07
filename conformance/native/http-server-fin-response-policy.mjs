import http from 'node:http';
import net from 'node:net';

async function run() {
  const server = http.createServer((_request, response) => {
    setTimeout(() => {
      response.end('late');
      server.close();
    }, 20);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));

  const response = await new Promise((resolve, reject) => {
    let received = '';
    const socket = net.connect(server.address().port, '127.0.0.1');
    socket.on('connect', () => {
      socket.end('GET / HTTP/1.1\r\nHost: localhost\r\n\r\n');
    });
    socket.on('data', chunk => { received += chunk.toString(); });
    socket.on('error', reject);
    socket.on('close', () => resolve(received));
  });
  console.log('default', JSON.stringify(response));
}

await run();
