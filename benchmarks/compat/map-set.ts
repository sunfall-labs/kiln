const m=new Map([['a',1],['b',2]]); m.set('a',3); const s=new Set([1,2,1]); console.log(JSON.stringify([Array.from(m),Array.from(s)]));
