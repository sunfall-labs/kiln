const x={a:[1,2]}; const y=structuredClone(x); y.a.push(3); console.log(JSON.stringify([x.a,y.a]));
