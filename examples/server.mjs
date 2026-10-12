import { createServer } from 'node:http';

const server = createServer((_request, response) => {
  response.end('Hello from kiln\n');
});
server.listen(3000);
process.on('SIGTERM', () => server.close(() => process.exit(0)));
