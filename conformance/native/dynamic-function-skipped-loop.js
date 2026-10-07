const run = new Function("methods", "let total = 0; if (methods) for (const name in methods) { if (name === 'x') { total += methods[name]; } else { total += 1; } } return total;");
console.log(run(null), run({ x: 3, y: 4 }));
const branch = new Function("yes", "let n = 0; if (yes) n = 1; else for (let i = 0; i < 3; i++) { n += i; } return n;");
console.log(branch(true), branch(false));
