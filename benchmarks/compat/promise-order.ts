const a=['sync']; Promise.resolve(21).then(x=>{a.push(String(x*2));console.log(JSON.stringify(a));});
