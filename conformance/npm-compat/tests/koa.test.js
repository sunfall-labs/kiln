// kiln eval smoke test (authored for kiln, not from porffor).
const Koa = require('koa');
const app = new Koa();
app.use(async (ctx, next) => { await next(); });
app.use(async ctx => { ctx.body = 'hi'; });
console.log(app.middleware.length);
console.log(typeof app.callback());
console.log(typeof app.listen);
const app2 = new Koa();
console.log(app2.middleware.length);
console.log(app.env);
