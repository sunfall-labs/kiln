const net = require('node:net');
// Reserve an ephemeral port, then close it before connecting so the refused connection is ours.
const server = net.createServer();
server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  server.close(() => {
    const socket = net.connect(port, '127.0.0.1');
    socket.on('error', error => {
      if (!(error instanceof Error) || typeof error.message !== 'string' || !error.message.length) {
        throw new Error('transport error must carry an Error with a message');
      }
      console.log('transport Error object');
      socket.destroy();
    });
  });
});
