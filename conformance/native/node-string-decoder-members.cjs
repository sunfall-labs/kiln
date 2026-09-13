// What `require("node:string_decoder")` publishes, and what StringDecoder answers.
const sd = require("node:string_decoder");

const names = Object.getOwnPropertyNames(sd).sort();
console.log(names.length);
for (const name of names) {
  console.log(name, typeof sd[name]);
}

const { StringDecoder } = sd;
console.log("constructor is function", typeof StringDecoder === "function");
console.log("self alias", StringDecoder.StringDecoder === StringDecoder);

const decoder = new StringDecoder("utf8");
console.log("encoding", decoder.encoding);

// UTF-8 multi-byte splitting across write calls
const p1 = decoder.write(new Uint8Array([0xf0, 0x9f]));
console.log("p1 empty", p1 === "");
const p2 = decoder.write(new Uint8Array([0x92, 0xa9, 0x21]));
console.log("p2 poop emoji", p2 === "💩!");

// end() with incomplete sequence produces replacement char
const p3 = decoder.end(new Uint8Array([0xe2]));
console.log("p3 replacement char", p3 === "\uFFFD");
