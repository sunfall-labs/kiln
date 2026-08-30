const fs=[]; for(let i=0;i<3;i++) fs.push(()=>i); console.log(fs.map(f=>f()).join());
