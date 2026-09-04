const chunks = [];
for (let i = 0; i < 1000; i++) {
  const buf = Buffer.from("data-payload-abcdefghijklmnopqrstuvwxyz".repeat(20));
  chunks.push(buf);
}

let totalLength = 0;
for (let j = 0; j < 100; j++) {
  const combined = Buffer.concat(chunks);
  totalLength += combined.length;
}

console.log("total bytes:", totalLength);
