const server = Kiln.serve({
  port: 3000,
  fetch(request: Request): Response {
    return new Response(`Hello, ${new URL(request.url).pathname}\n`);
  },
});

process.on('SIGTERM', () => server.close(() => process.exit(0)));
