const net = require('node:net');
function removedListener() {
  throw new Error('constructor connection listener must retain its identity');
}
const server = net.createServer(removedListener);
server.removeListener('connection', removedListener);
server.prependListener('connection', socket => {
  if (typeof socket.cork !== 'function' || typeof socket.pause !== 'function' || typeof socket.resume !== 'function') {
    throw new Error('host must decorate socket before any connection listener');
  }
  socket.end('ok');
});
server.listen(0, '127.0.0.1', () => {
  let data = '';
  const socket = net.connect(server.address().port, '127.0.0.1');
  socket.on('data', chunk => { data += chunk.toString(); });
  socket.on('end', () => {
    if (data !== 'ok') throw new Error('unexpected socket payload');
    console.log('socket facade preserves listener identity');
    server.close();
  });
});
