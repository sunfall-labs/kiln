const fs = require('node:fs');
const path = require('node:path');

const file = path.join(__dirname, `.kiln-fs-callback-${process.pid}.tmp`);
let returned = false;
fs.open(file, 'w+', (openError, fd) => {
  if (openError) throw openError;
  if (!returned || typeof fd !== 'number') throw new Error('open callback ran synchronously');
  fs.write(fd, 'hi', 'utf8', (writeError, written, text) => {
    if (writeError) throw writeError;
    if (written !== 2 || text !== 'hi') throw new Error('string write callback differs');
    const data = Buffer.from('42');
    fs.write(fd, data, (bufferError, bufferWritten, buffer) => {
      if (bufferError) throw bufferError;
      if (bufferWritten !== 2 || buffer !== data) throw new Error('buffer write callback differs');
      fs.fsync(fd, (syncError) => {
        if (syncError) throw syncError;
        fs.close(fd, (closeError) => {
          if (closeError) throw closeError;
          const actual = fs.readFileSync(file, 'utf8');
          fs.unlinkSync(file);
          if (actual !== 'hi42') throw new Error(`wrote ${actual}`);
          console.log('pass');
        });
      });
    });
  });
});
returned = true;
