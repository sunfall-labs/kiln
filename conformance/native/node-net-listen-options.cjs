'use strict';
const net = require('node:net');
const server = net.createServer();
let called = 0;
const options = Object.create({ port: 0 });
Object.defineProperty(options, 'host', { get() { return '127.0.0.1'; } });
server.listen(options, () => {
  called++;
  const address = server.address();
  console.log(address.address, address.port > 0, called);
  server.close(() => console.log('closed'));
});
for (const bad of [{}, { port: false }, { port: -1 }, { port: 65536 }]) {
  try { net.createServer().listen(bad); console.log('unexpected'); }
  catch (error) { console.log(error.name); }
}
