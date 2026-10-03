const http = require('node:http');
const server = http.createServer((request, response) => {
  response.writeHead(302, request.url === '/' ? undefined : 'Found', {
    Location: '/target', 'X-Overload': 'third'
  });
  response.end();
});
server.listen(0, '127.0.0.1', () => {
  http.get({ hostname: '127.0.0.1', port: server.address().port }, (response) => {
    console.log(response.statusCode, response.headers.location, response.headers['x-overload']);
    response.resume();
    response.on('end', () => {
      http.get({ hostname: '127.0.0.1', port: server.address().port, path: '/named' }, (named) => {
        console.log(named.statusCode, named.headers.location, named.headers['x-overload']);
        named.resume();
        named.on('end', () => server.close());
      });
    });
  });
});
