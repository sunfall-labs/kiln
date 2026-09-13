// What `require("node:crypto")` publishes, and what functions answer.
const crypto = require("node:crypto");

const names = Object.getOwnPropertyNames(crypto).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof crypto[name]);
}

console.log("default self-link:", crypto.default === crypto);
console.log("getHashes:", crypto.getHashes().join(","));

const hash = crypto.createHash("sha256").update("hello world").digest("hex");
console.log("sha256 match:", hash === "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");

const hmac = crypto.createHmac("sha256", "key").update("The quick brown fox jumps over the lazy dog").digest("hex");
console.log("hmac match:", hmac === "f7bc83f430538424b13298e6aa6fb143ef4d59a14946175997479dbc2d1a3cd8");

const md5Hash = crypto.createHash("md5").update("hello world").digest("hex");
console.log("md5 match:", md5Hash === "5eb63bbbe01eeed093cb22bb8f5acdc3");

const sha1Hash = crypto.createHash("sha1").update("hello world").digest("hex");
console.log("sha1 match:", sha1Hash === "2aae6c35c94fcfb415dbe95f408b9ce91ee846ed");

const uuid = crypto.randomUUID();
console.log("uuid valid:", typeof uuid === "string" && uuid.length === 36 && uuid[14] === "4");

const bytes = crypto.randomBytes(16);
console.log("randomBytes len:", bytes.length);

const buf = new Uint8Array(8);
crypto.randomFillSync(buf, 2, 4);
console.log("randomFillSync zeros:", buf[0] === 0 && buf[1] === 0 && buf[6] === 0 && buf[7] === 0);

const arr = new Uint8Array(16);
const res = crypto.getRandomValues(arr);
console.log("getRandomValues len:", res.length);

const b1 = Buffer.from([1, 2, 3, 4]);
const b2 = Buffer.from([1, 2, 3, 4]);
const b3 = Buffer.from([1, 2, 3, 5]);
console.log("timingSafeEqual eq:", crypto.timingSafeEqual(b1, b2));
console.log("timingSafeEqual neq:", crypto.timingSafeEqual(b1, b3));
