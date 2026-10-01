const net = require('node:net');

const socket = net.connect({ host: '127.0.0.1', port: 0 });
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
