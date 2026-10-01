const net = require('node:net');

const server = net.createServer(() => {});
server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => {
    socket.cork();
    socket.write('discard', error => {
      console.log('write callback', error && error.code);
    });
    socket.end(function (error) {
      'use strict';
      if (this === socket || arguments.length !== 1 || error?.code !== 'ERR_STREAM_DESTROYED') {
        throw new Error('wrong destroyed end callback arguments');
      }
      console.log('end callback', this === socket, arguments.length, error.code);
    });
    socket.destroy();
  });
  socket.on('close', () => {
    console.log('closed');
    server.close();
  });
});
