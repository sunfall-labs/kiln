const net = require('node:net');

function check(corked, expectedBytes, expectedCallbacks, done) {
  const callbacks = [];
  let peerEnded = false;
  let writerEnded = false;
  let bytes = '';
  let completed = false;
  function complete() {
    if (!peerEnded || !writerEnded || completed) return;
    completed = true;
    if (bytes !== expectedBytes || callbacks.join(',') !== expectedCallbacks) {
      throw new Error(`write order changed: ${bytes}:${callbacks.join(',')}`);
    }
    console.log(JSON.stringify(bytes), callbacks.join(','));
    server.close(done);
  }
  const server = net.createServer(socket => {
    socket.on('data', chunk => bytes += chunk.toString());
    socket.on('end', () => { peerEnded = true; complete(); });
  });
  server.listen(0, '127.0.0.1', () => {
    const socket = net.connect(server.address().port, '127.0.0.1');
    socket.on('connect', () => {
      if (corked) socket.cork();
      else socket.write('a', () => callbacks.push('first'));
      socket.write('', function (error) {
        'use strict';
        if (arguments.length !== 1 || error !== null) throw new Error('empty write callback arguments');
        callbacks.push('empty');
      });
      if (!corked) socket.write('b', () => callbacks.push('second'));
      socket.end(corked ? undefined : 'c', () => {
        callbacks.push('end');
        writerEnded = true;
        complete();
      });
    });
  });
}
check(false, 'abc', 'first,empty,second,end', () => {
  check(true, '', 'empty,end', () => {});
});
