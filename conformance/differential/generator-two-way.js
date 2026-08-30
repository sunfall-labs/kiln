function* g(){ const x = yield 1; console.log("got",x); return 2 } const it=g(); console.log(it.next().value); console.log(it.next(9));
