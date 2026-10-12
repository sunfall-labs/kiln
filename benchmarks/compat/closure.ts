function counter() { let n=0; return () => ++n; } const f=counter(); console.log(JSON.stringify([f(),f(),f()]));
