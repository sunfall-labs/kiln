const fs = require('node:fs');
const path = require('node:path');
const pino = require('pino');

const destination = path.join(__dirname, `pino-output-${process.pid}.log`);
if (fs.existsSync(destination)) fs.unlinkSync(destination);
const transport = pino.transport({ target: 'pino/file', options: { destination } });
const logger = pino({ base: null, timestamp: false }, transport);
logger.info({ answer: 42 }, 'worker transport');
transport.flushSync();
transport.end();
transport.on('close', () => {
  const line = fs.readFileSync(destination, 'utf8').trim();
  fs.unlinkSync(destination);
  console.log(line);
});
