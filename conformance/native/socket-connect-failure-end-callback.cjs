const net = require('node:net');

const server = net.createServer();
server.listen(0, '127.0.0.1', () => {
  const port = server.address().port;
  server.close(() => {
    const socket = net.connect({ host: '127.0.0.1', port });
    const events = [];
    let endError;
    let socketError;
    socket.on('error', error => {
      if (error.code !== 'ECONNREFUSED') throw new Error('wrong socket error');
      socketError = error;
      events.push('error');
      console.log('error', error.code);
    });
    socket.on('close', () => {
      if (events.join(',') !== 'end,error') throw new Error(`wrong callback order: ${events}`);
      if (socketError !== endError) throw new Error('end callback did not share socket error');
      console.log('close');
    });
    socket.end(function (error) {
      'use strict';
      if (this === socket || arguments.length !== 1 || error?.code !== 'ECONNREFUSED') {
        throw new Error('wrong refused end callback arguments');
      }
      endError = error;
      events.push('end');
      console.log('end callback', this === socket, arguments.length, error.code);
    });
  });
});
