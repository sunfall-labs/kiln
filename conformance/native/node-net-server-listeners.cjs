const net = require('node:net');
const socket = {};
const server = net.createServer(function (actual) {
  console.log('constructor listener', this === server, actual === socket);
});
server.on('connection', function (actual) {
  console.log('added listener', this === server, actual === socket);
});
server.emit('connection', socket);
const withoutListener = net.createServer();
withoutListener.on('connection', actual => console.log('optional listener', actual === socket));
withoutListener.emit('connection', socket);
