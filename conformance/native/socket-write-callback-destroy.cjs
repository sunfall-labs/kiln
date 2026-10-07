const net = require('node:net');

const server = net.createServer((socket) => {
  socket.on('data', () => {});
  socket.on('close', () => server.close());
});

server.listen(0, '127.0.0.1', () => {
  const client = net.connect(server.address().port, '127.0.0.1');
  client.on('close', () => console.log('closed'));
  client.write('payload', () => {
    console.log('callback');
    client.destroy();
  });
});
