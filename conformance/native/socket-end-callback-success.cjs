const net = require('node:net');

const server = net.createServer(socket => socket.resume());
server.listen(0, '127.0.0.1', () => {
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('connect', () => {
    socket.end(function (error) {
      'use strict';
      if (this === socket || arguments.length !== 1 || error !== null) {
        throw new Error('wrong successful end callback arguments');
      }
      console.log('end callback', this === socket, arguments.length, error === null);
      server.close();
    });
  });
});
