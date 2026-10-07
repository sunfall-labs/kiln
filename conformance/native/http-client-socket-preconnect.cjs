const http = require('node:http');

const server = http.createServer((_request, response) => {
  response.end('ok');
});

server.listen(0, '127.0.0.1', () => {
  const request = http.get(`http://127.0.0.1:${server.address().port}`);
  request.on('socket', socket => {
    console.log(`socket:${socket.connecting}:${request.socket === socket}:${request.connection === socket}`);
    socket.once('connect', () => console.log(`connect:${socket.connecting}`));
  });
  request.on('response', response => {
    response.resume();
    response.on('end', () => server.close(() => console.log('response:ok')));
  });
});
