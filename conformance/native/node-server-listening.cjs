'use strict';
const net = require('node:net');
const http = require('node:http');

let remaining = 3;
function checkServer(name, server) {
  if (server.listening !== false) throw new Error(`${name}: listening must start false`);
  if (name === 'net.createServer') server.address = () => null;
  server.listen(0, '127.0.0.1', () => {
    if (server.listening !== true) throw new Error(`${name}: listening must be true after bind`);
    server.close(() => {
      if (server.listening !== false) throw new Error(`${name}: listening must be false after close`);
      console.log(`${name}: listening transitions ok`);
      remaining--;
      if (remaining === 0) console.log('all server listening transitions ok');
    });
  });
  if (server.listening !== false) throw new Error(`${name}: listening must remain false before bind`);
}

checkServer('net.createServer', net.createServer());
checkServer('http.createServer', http.createServer());
checkServer('new http.Server', new http.Server());
