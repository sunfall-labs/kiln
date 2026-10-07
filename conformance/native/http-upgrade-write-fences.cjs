const http = require('node:http');
const net = require('node:net');

const server = http.createServer();
server.on('upgrade', (_request, socket) => {
  socket.write('HTTP/1.1 101 Switching Protocols\r\nConnection: Upgrade\r\nUpgrade: fence\r\n\r\n');
  socket.write('one', () => console.log('write:one'));
  socket.write('two', () => {
    console.log('write:two');
    socket.end();
    server.close();
  });
});

const port = 19839;
server.listen(port, '127.0.0.1', () => {
  const client = net.connect({ host: '127.0.0.1', port });
  const chunks = [];
  client.on('connect', () => client.write(
    'GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\nUpgrade: fence\r\n\r\n',
  ));
  client.on('data', chunk => chunks.push(chunk));
  client.on('end', () => {
    const bytes = Buffer.concat(chunks);
    const text = bytes.toString('latin1');
    const split = text.indexOf('\r\n\r\n');
    if (!text.startsWith('HTTP/1.1 101') || split < 0 || text.slice(split + 4) !== 'onetwo') {
      throw new Error('upgrade write fences changed handshake or payload ordering');
    }
    console.log('payload:onetwo');
    client.destroy();
  });
});
