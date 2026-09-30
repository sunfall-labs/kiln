const net = require('node:net');
const server = net.createServer(socket => {
  socket.on('data', data => socket.end(data));
});
server.listen(0, '127.0.0.1', () => {
  const client = net.connect({ host: '127.0.0.1', port: server.address().port });
  const received = [];
  client.on('connect', () => client.write(Buffer.from([0, 1, 127, 128, 255])));
  client.on('data', data => received.push(data));
  client.on('end', () => {
    console.log(Buffer.concat(received).toString('hex'));
    client.end();
    server.close();
  });
});
