const net = require('node:net');

const server = net.createServer((socket) => socket.resume());
server.listen(0, '127.0.0.1', () => {
  const verifyCancelled = (label) => function (error) {
    if (arguments.length !== 1 || error?.code !== 'ECANCELED') {
      throw new Error(`wrong ${label} callback: ${error?.code}`);
    }
    console.log(label, error.code);
  };

  const run = (withActiveCallback, done) => {
    const socket = net.connect(server.address().port, '127.0.0.1');
    socket.on('connect', () => {
      const activeCallback = withActiveCallback ? verifyCancelled('active') : undefined;
      const accepted = socket.write(Buffer.alloc(16 * 1024 * 1024), activeCallback);
      if (accepted !== false) throw new Error('large write unexpectedly avoided backpressure');
      console.log('write ready', accepted);
      socket.write('q', verifyCancelled('queued'));
      socket.end(verifyCancelled('end'));
      socket.destroy();
    });
    socket.on('close', done);
  };

  run(true, () => {
    run(false, () => {
      console.log('closed');
      server.close();
    });
  });
});
