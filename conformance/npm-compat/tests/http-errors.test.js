// kiln eval smoke test (authored for kiln, not from porffor).
const createError = require('http-errors');
const e404 = createError(404);
console.log(e404.status, e404.message);
const e500 = createError(500, 'boom');
console.log(e500.status, e500.message, e500.expose);
console.log(createError.isHttpError(e404), createError.isHttpError(new Error('x')));
console.log(createError(400).expose);
console.log(createError(418).message);
