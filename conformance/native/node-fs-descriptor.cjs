const fs = require('node:fs');
const filename = `${process.env.TMPDIR || '/tmp'}/kiln-descriptor-${process.pid}.txt`;
const fd = fs.openSync(filename, 'w');
console.log(fs.writeSync(fd, 'worker'));
fs.fsyncSync(fd);
fs.closeSync(fd);
console.log(fs.readFileSync(filename, 'utf8'));
fs.unlinkSync(filename);
