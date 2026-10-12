const x=JSON.parse('{"n":21,"drop":1}',(k,v)=>k==='drop'?undefined:k==='n'?v*2:v); console.log(JSON.stringify(x));
