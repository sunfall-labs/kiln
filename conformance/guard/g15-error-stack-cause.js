const inner = new Error('inner');
const e = new Error('outer', { cause: inner });
console.log(typeof e.stack, e.cause === inner, typeof Error.captureStackTrace);
