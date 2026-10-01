const net = require('node:net');

const server = net.createServer(socket => {
  socket.on('data', () => {});
});
server.listen(19840, '127.0.0.1', () => {
  const socket = net.connect({ host: '127.0.0.1', port: 19840 }, () => {
    socket.cork();
    const ready = socket.write(Buffer.alloc(16 * 1024));
    if (ready !== false) throw new Error('a corked write at the high water mark returned true');
    socket.uncork();
    socket.end();
    server.close();
    console.log('corked backpressure honored');
  });
});
