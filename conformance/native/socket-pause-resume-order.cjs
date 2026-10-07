const net = require('node:net');

const server = net.createServer(socket => {
  socket.write('first');
  setTimeout(() => socket.end('second'), 5);
});

server.listen(0, '127.0.0.1', () => {
  let received = '';
  const socket = net.connect(server.address().port, '127.0.0.1', () => {
    socket.pause();
    setTimeout(() => socket.resume(), 10);
  });
  socket.on('data', chunk => {
    received += chunk.toString();
  });
  socket.on('end', () => {
    if (received !== 'firstsecond') {
      throw new Error(`paused socket lost or reordered bytes: ${received}`);
    }
    console.log('paused bytes resumed in order');
    server.close();
  });
});
