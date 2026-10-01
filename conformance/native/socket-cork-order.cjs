const net = require('node:net');

const server = net.createServer((socket) => {
  const chunks = [];
  socket.on('data', (chunk) => chunks.push(chunk.toString()));
  socket.on('end', () => {
    console.log(chunks.join(''));
    server.close();
  });
});

server.listen(0, '127.0.0.1', () => {
  const client = net.connect(server.address().port, '127.0.0.1', () => {
    client.cork();
    client.write('first:');
    client.write('second');
    client.uncork();
    client.end();
  });
});
