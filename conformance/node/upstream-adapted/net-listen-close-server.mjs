import { createServer } from 'node:net';

const server = createServer(() => {
  throw new Error('closed server accepted a connection');
});
server.on('error', error => { throw error; });
server.on('listening', () => { throw new Error('closed server started listening'); });
server.on('close', () => {
  if (server.address() !== null) throw new Error('closed server retained an address');
  console.log('pass');
});
server.listen(0, '127.0.0.1');
server.close();
