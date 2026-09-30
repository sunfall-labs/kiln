// kiln eval smoke test (authored for kiln, not from porffor).
const pino = require('pino');

const chunks = [];
const stream = { write: (s) => { chunks.push(JSON.parse(s)); } };
const logger = pino({ level: 'info' }, stream);

logger.info({ a: 1 }, 'hello');
logger.debug('hidden-below-level');
logger.error(new Error('boom'));

console.log(chunks.length);
console.log(chunks[0].msg, chunks[0].level, chunks[0].a);
console.log(chunks[1].err.message, chunks[1].level);

const child = logger.child({ req: 'x1' });
child.warn('from child');
console.log(chunks[2].req, chunks[2].msg, chunks[2].level);
