const net = require('node:net');

const server = net.createServer(() => {});
server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => {
    socket.cork();
    socket.write('discard', error => {
      console.log('write callback', error && error.code);
    });
    socket.destroy();
  });
  socket.on('close', () => {
    console.log('closed');
    server.close();
  });
});
