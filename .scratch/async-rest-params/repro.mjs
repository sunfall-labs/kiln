const target = { m(x) { return x + 1; } };
const direct = async function (...a) { return target.m.apply(target, a); };
const noRest = async function (a) { return target.m.apply(target, [a]); };
const sync = function (...a) { return target.m.apply(target, a); };
Promise.all([direct(1), noRest(1), Promise.resolve(sync(1))]).then((v) => console.log(v.join(",")));
