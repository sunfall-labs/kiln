const keys = [];
for (const k in Buffer) keys.push(k);
console.log("for-in:", keys.join(",") || "(none)");
console.log("hasOwn isBuffer:", Buffer.hasOwnProperty("isBuffer"));
console.log("typeof isBuffer:", typeof Buffer.isBuffer);
console.log("getOwnPropertyNames:", Object.getOwnPropertyNames(Buffer).join(",") || "(none)");
