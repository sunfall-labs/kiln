const net = require('node:net');
const callbacks = [];

const server = net.createServer((socket) => {
  const chunks = [];
  socket.on('data', (chunk) => chunks.push(chunk.toString()));
  socket.on('end', () => {
    if (chunks.join('') !== 'abc') throw new Error('write byte order changed');
    if (callbacks.join(',') !== 'first,empty,second,end') {
      throw new Error(`write callback order changed: ${callbacks.join(',')}`);
    }
    console.log(chunks.join(''), callbacks.join(','));
    server.close();
  });
});

server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => {
    socket.write('a', () => callbacks.push('first'));
    socket.write('', () => callbacks.push('empty'));
    socket.write('b', () => callbacks.push('second'));
    socket.end('c', () => callbacks.push('end'));
  });
});
