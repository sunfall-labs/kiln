import { createServer } from 'node:net';

const server = createServer(() => {
  throw new Error('closed server accepted a connection');
});
let listening = false;
server.on('error', error => { throw error; });
server.on('listening', () => {
  listening = true;
  const address = server.address();
  if (address.address !== '127.0.0.1' || address.port <= 0) {
    throw new Error('listener did not expose its bound address');
  }
  server.close();
});
server.on('close', () => {
  if (!listening) throw new Error('server closed before it listened');
  console.log('pass');
});
server.listen(0, '127.0.0.1');
