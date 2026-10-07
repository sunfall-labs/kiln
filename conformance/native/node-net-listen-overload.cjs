'use strict';
const net = require('node:net');
const server = net.createServer(() => {});
let callbackSeen = false;
let eventSeen = false;
function finish() {
  if (!callbackSeen || !eventSeen) return;
  const address = server.address();
  if (!address || typeof address !== 'object' || address.port === 0) {
    throw new Error('listen completion ran before the bound address was available');
  }
  console.log('listen overload and bound event: ok');
  server.close();
}
const observer = {
  onListening(event) {
    if (event !== 'listening') throw new Error('unexpected listening event argument');
    eventSeen = true;
    finish();
  },
};
server.on('listening', observer.onListening.bind(observer, 'listening'));
server.listen(0, '127.0.0.1', 1, () => {
  callbackSeen = true;
  finish();
});
