const http = require('node:http');
const net = require('node:net');

const server = http.createServer((_request, response) => {
  response.writeHead(200, { 'content-length': '2' });
  response.end('ok');
});

server.on('upgrade', (_request, socket, head) => {
  const received = [head];
  let length = head.length;
  let replied = false;
  const finish = () => {
    if (replied || length < 3) return;
    replied = true;
    const bytes = Buffer.concat(received).subarray(0, 3);
    socket.write(bytes);
    socket.end();
    server.close();
  };
  socket.on('data', chunk => {
    received.push(chunk);
    length += chunk.length;
    finish();
  });
  socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: kiln-probe\r\n\r\n');
  finish();
});

server.listen(0, '127.0.0.1', () => {
  const client = net.connect({ host: '127.0.0.1', port: server.address().port });
  const response = [];
  client.on('connect', () => {
    client.write(Buffer.from([
      ...Buffer.from('GET /ordinary HTTP/1.1\r\nHost: localhost\r\n\r\n'),
      ...Buffer.from('GET /upgrade HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive, Upgrade\r\nUpgrade: kiln-probe\r\n\r\n'),
      0x00, 0xa5, 0xff,
    ]));
  });
  client.on('data', chunk => response.push(chunk));
  client.on('end', () => {
    const bytes = Buffer.concat(response);
    const text = bytes.toString('latin1');
    const raw = bytes.lastIndexOf(Buffer.from([0x00, 0xa5, 0xff]));
    if (!text.includes('200') || !text.includes('101') || raw < 0) {
      throw new Error('pipelined HTTP upgrade did not preserve both responses and raw head bytes');
    }
    console.log(`200,101,${bytes.subarray(raw, raw + 3).toString('hex')}`);
    client.destroy();
  });
});
