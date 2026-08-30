let x=1; { let x=2; } console.log(x); const arr=[]; for (const y of [1,2]) arr.push(()=>y); console.log(arr.map(f=>f()).join());
