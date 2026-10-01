const net = require('node:net');

const server = net.createServer();
server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  server.close(() => {
    const socket = net.connect({ host: '127.0.0.1', port });
    socket.on('error', error => console.log('error', error.code));
    socket.on('close', () => console.log('close'));
    socket.end(() => console.log('end callback'));
  });
});
