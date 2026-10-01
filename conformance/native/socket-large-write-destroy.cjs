const net = require('node:net');

const server = net.createServer((socket) => socket.resume());
server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => {
    const accepted = socket.write(Buffer.alloc(16 * 1024 * 1024), function (error) {
      if (arguments.length !== 1 || error?.code !== 'ECANCELED') {
        throw new Error(`wrong canceled write callback: ${error?.code}`);
      }
      console.log('write callback', error.code);
    });
    if (accepted !== false) throw new Error('large write unexpectedly avoided backpressure');
    console.log('write ready', accepted);
    socket.end(function (error) {
      if (arguments.length !== 1 || error?.code !== 'ERR_STREAM_DESTROYED') {
        throw new Error(`wrong destroyed end callback: ${error?.code}`);
      }
      console.log('end callback', error.code);
    });
    socket.destroy();
  });
  socket.on('close', () => {
    console.log('closed');
    server.close();
  });
});
