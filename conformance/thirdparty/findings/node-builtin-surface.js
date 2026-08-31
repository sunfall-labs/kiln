// The `--node` builtin modules are partial stubs with no enumerable exports.
//
// `Object.keys` answers 0 for every one of them, so `{ ...require("path") }` and every
// `export * from "path"` re-export shim produces an empty object. Individual members are missing
// outright: this is what stops the TypeScript compiler (`os.platform` in `getNodeSystem`) and
// postcss (`path.dirname` in `PreviousMap`).
//
// node:  os object keys 23 platform function EOL "\n" tmpdir function
//        path function function function string "/"  |  util function function object function
//        fs function function function  |  crypto function function function
// kiln:  os object keys 0 platform undefined EOL undefined tmpdir undefined
//        path function function undefined string "/"  |  util function undefined undefined undefined
//        fs undefined function undefined  |  crypto function function undefined
const os = require("os");
const path = require("path");
const util = require("util");
const fs = require("fs");
const crypto = require("crypto");
console.log("os", typeof os, "keys", Object.keys(os).length, "platform", typeof os.platform, "EOL", JSON.stringify(os.EOL), "tmpdir", typeof os.tmpdir);
console.log("path", typeof path.join, typeof path.resolve, typeof path.dirname, typeof path.sep, JSON.stringify(path.sep));
console.log("util", typeof util.inspect, typeof util.promisify, typeof util.types, typeof util.TextDecoder);
console.log("fs", typeof fs.readFileSync, typeof fs.existsSync, typeof fs.statSync);
console.log("crypto", typeof crypto.randomUUID, typeof crypto.createHash, typeof crypto.randomBytes);
console.log("getOwnPropertyNames-os", Object.getOwnPropertyNames(os).length);
