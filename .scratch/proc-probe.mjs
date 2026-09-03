console.log("bare typeof:", typeof process);
console.log("bare cwd:", typeof process.cwd === "function");
console.log("globalThis.process:", typeof globalThis.process);
console.log("in globalThis:", "process" in globalThis);
