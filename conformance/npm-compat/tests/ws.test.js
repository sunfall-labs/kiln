// kiln eval smoke test (authored for kiln, not from porffor).
const WebSocket = require('ws');

console.log(WebSocket.OPEN, WebSocket.CLOSED);

const server = new WebSocket.Server({ port: 0 });
server.on('listening', () => {
  const port = server.address().port;
  server.on('connection', (ws) => {
    ws.on('message', (m) => {
      console.log('server got:', m.toString());
      ws.send('hi-from-server');
    });
  });

  const client = new WebSocket(`ws://127.0.0.1:${port}`);
  client.on('open', () => client.send('hello-from-client'));
  client.on('message', (m) => {
    console.log('client got:', m.toString());
    client.close();
    server.close();
  });
});
