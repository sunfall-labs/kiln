const p={get answer(){return this.base+2;}}; const x=Object.create(p); x.base=40; console.log(JSON.stringify([x.answer,Object.getPrototypeOf(x)===p]));
