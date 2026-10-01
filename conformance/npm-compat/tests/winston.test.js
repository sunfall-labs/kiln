// kiln eval smoke test (authored for kiln, not from porffor).
const winston = require('winston');

const logger = winston.createLogger({
  level: 'info',
  format: winston.format.printf(({ level, message }) => `${level}: ${message}`),
  transports: [new winston.transports.Console({ silent: true })]
});
console.log('level:', logger.level);
logger.level = 'debug';
console.log('level set:', logger.level);

console.log('format combine:', typeof winston.format.combine(winston.format.label({ label: 'x' })) === 'object');
console.log('json format:', winston.format.json().transform({ level: 'info', message: 'm' }, {}).message === 'm');

const mem = [];
const memTransport = new winston.transports.Console({ silent: true });
const logger2 = winston.createLogger({ transports: [memTransport] });
logger2.on('data', (info) => mem.push(info.level + ':' + info.message));
logger2.info('hello');
setImmediate(() => {
  console.log('logged:', JSON.stringify(mem));
  console.log('transports count:', logger2.transports.length);
});
