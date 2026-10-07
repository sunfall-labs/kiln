const net = require('node:net');
const server = net.createServer(socket => {
  if (!socket.readable || !socket.writable || socket.destroyed) throw new Error('accepted socket must be duplex');
  socket.on('data', data => socket.end(data));
});
server.listen(19837, '127.0.0.1', () => {
  const client = net.connect({ host: '127.0.0.1', port: 19837 });
  const received = [];
  client.on('close', () => {
    if (client.readable || client.writable || !client.destroyed) throw new Error('closed socket must expose terminal state');
  });
  client.on('connect', () => client.write(Buffer.from([0, 1, 127, 128, 255])));
  client.on('data', data => received.push(data));
  client.on('end', () => {
    console.log(Buffer.concat(received).toString('hex'));
    client.end();
    server.close();
  });
});
