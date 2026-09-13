// Parity test for node:os TypeScript built-in between interpreter and native backend.
const os = require("os");

console.log("typeof EOL", typeof os.EOL, os.EOL === "\n");
console.log("typeof devNull", typeof os.devNull, os.devNull);
console.log("typeof arch", typeof os.arch, typeof os.arch());
console.log("typeof platform", typeof os.platform, typeof os.platform());
console.log("typeof type", typeof os.type, typeof os.type());
console.log("typeof endianness", typeof os.endianness, os.endianness());
console.log("typeof homedir", typeof os.homedir, typeof os.homedir());
console.log("typeof tmpdir", typeof os.tmpdir, typeof os.tmpdir());
console.log("typeof hostname", typeof os.hostname, typeof os.hostname());
console.log("typeof uptime", typeof os.uptime, typeof os.uptime());
console.log("typeof freemem", typeof os.freemem, typeof os.freemem());
console.log("typeof totalmem", typeof os.totalmem, typeof os.totalmem());
console.log("typeof loadavg", typeof os.loadavg, Array.isArray(os.loadavg()));
console.log("typeof cpus", typeof os.cpus, Array.isArray(os.cpus()));
console.log("typeof networkInterfaces", typeof os.networkInterfaces, typeof os.networkInterfaces());
console.log("typeof userInfo", typeof os.userInfo, typeof os.userInfo());
console.log("typeof availableParallelism", typeof os.availableParallelism, typeof os.availableParallelism());
