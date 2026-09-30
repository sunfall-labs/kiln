const net = require('node:net');
const server = net.createServer(socket => {
  socket.on('data', data => socket.write(data));
});
server.listen(0, '127.0.0.1', () => {
  const client = net.connect({ host: '127.0.0.1', port: server.address().port });
  client.on('connect', () => client.write('options'));
  client.on('data', data => {
    console.log(data.toString());
    client.end();
    server.close();
  });
});
