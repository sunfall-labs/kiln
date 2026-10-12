const x=new Proxy({answer:1},{get(t,k){return k==='answer'?42:t[k];}}); console.log(JSON.stringify(x.answer));
