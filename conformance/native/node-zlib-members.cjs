// What `require("node:zlib")` publishes, and what functions answer.
const zlib = require("node:zlib");

const names = Object.getOwnPropertyNames(zlib).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof zlib[name]);
}

console.log("default self-link:", zlib.default === zlib);
console.log("constants Z_OK:", zlib.constants.Z_OK === 0);
console.log("constants Z_SYNC_FLUSH:", zlib.constants.Z_SYNC_FLUSH === 2);
console.log("constants Z_STREAM_END:", zlib.constants.Z_STREAM_END === 1);

const def = zlib.deflateSync("hello zlib conformance probe");
const inf = zlib.inflateSync(def);
console.log("deflate+inflate roundtrip:", inf.toString() === "hello zlib conformance probe");

const gz = zlib.gzipSync("hello gzip conformance probe");
const ungz = zlib.gunzipSync(gz);
console.log("gzip+gunzip roundtrip:", ungz.toString() === "hello gzip conformance probe");

const unzGz = zlib.unzipSync(gz);
const unzDef = zlib.unzipSync(def);
console.log("unzip gz roundtrip:", unzGz.toString() === "hello gzip conformance probe");
console.log("unzip def roundtrip:", unzDef.toString() === "hello zlib conformance probe");

const stream = zlib.createGzip();
stream.end(Buffer.from("streaming probe test"));
const outGz = stream.read();
const unstream = zlib.createGunzip();
unstream.end(outGz);
console.log("streaming roundtrip:", unstream.read().toString() === "streaming probe test");

let brotliThrew = false;
try {
  zlib.createBrotliDecompress().end("hi");
} catch (e) {
  brotliThrew = true;
}
console.log("brotli threw:", brotliThrew);
