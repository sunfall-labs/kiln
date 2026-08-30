function F(){} F.prototype.m=function(){return 1}; const f=new F(); console.log(f.m(), f instanceof F, Object.getPrototypeOf(f)===F.prototype);
