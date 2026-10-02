const net = require('node:net');

// Connecting to port 0 is refused on Linux but fails with EADDRNOTAVAIL on macOS, in node as
// well as here. Reserve a free port and release it: nothing listens there on any platform.
const probe = net.createServer();
probe.listen(0, '127.0.0.1', () => {
  const port = probe.address().port;
  probe.close(() => {
    const socket = net.connect({ host: '127.0.0.1', port });
    const events = [];
    socket.on('error', error => {
      if (error.code !== 'ECONNREFUSED') throw new Error('wrong socket error');
      events.push('error');
    });
    socket.write('queued', function (error) {
      'use strict';
      if (this === socket || arguments.length !== 1 ||
          error?.code !== 'ERR_SOCKET_CLOSED_BEFORE_CONNECTION') {
        throw new Error('wrong failed write callback arguments');
      }
      events.push('write');
    });
    socket.end(function (error) {
      'use strict';
      if (this === socket || arguments.length !== 1 || error?.code !== 'ECONNREFUSED') {
        throw new Error('wrong refused end callback arguments');
      }
      events.push('end');
      if (events.join(',') !== 'error,write,end') {
        throw new Error(`wrong callback order: ${events}`);
      }
      console.log(events.join(' '));
    });
  });
});
