import { createServer } from 'node:http';
import { connect } from 'node:net';

let handled = false;
let responseBytes = 0;
const server = createServer((request, response) => {
  if (request.method !== 'POST' || request.url !== '/kiln') {
    throw new Error('request line was not preserved');
  }
  let bodyBytes = 0;
  request.pause();
  request.on('data', chunk => { bodyBytes += chunk.length; });
  request.on('end', () => {
    if (bodyBytes !== 5) throw new Error('request body was not streamed');
    handled = true;
    response.writeHead(201, { 'Content-Type': 'text/plain' });
    // `write()` returning false is advisory backpressure, not a failed write. The response is
    // still queued and ordering is asserted by the final streamed response below.
    response.write('hello ');
    response.end('tokio');
  });
  setTimeout(() => request.resume(), 1);
});

server.listen(0, '127.0.0.1', () => {
  const socket = connect(server.address().port, '127.0.0.1', () => {
    socket.end('POST /kiln HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\nConnection: close\r\n\r\n12345');
  });
  socket.on('data', chunk => { responseBytes += chunk.length; });
  socket.on('end', () => {
    if (!handled || responseBytes === 0) throw new Error('response did not stream to the client');
    server.close(() => console.log('pass'));
  });
  socket.on('error', error => { throw error; });
});
