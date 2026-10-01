const net = require('node:net');

const server = net.createServer(socket => {
  socket.on('data', () => {});
});
server.listen(19840, '127.0.0.1', () => {
  const socket = net.connect({ host: '127.0.0.1', port: 19840 }, () => {
    socket.cork();
    // This exceeds the byte-stream default on both Node's 16 KiB and 64 KiB profiles.
    // The probe checks cork buffering participates in backpressure, independently of defaults.
    const ready = socket.write(Buffer.alloc(64 * 1024));
    if (ready !== false) throw new Error('a large corked write failed to apply backpressure');
    socket.uncork();
    socket.end();
    server.close();
    console.log('corked backpressure honored');
  });
});
