// What `require("node:console")` publishes, and what functions answer.
const consoleModule = require("node:console");

const names = Object.getOwnPropertyNames(consoleModule).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof consoleModule[name]);
}

console.log("default self-link:", consoleModule.default === consoleModule);
console.log("Console is fn:", typeof consoleModule.Console === "function");
console.log("log is fn:", typeof consoleModule.log === "function");
console.log("error is fn:", typeof consoleModule.error === "function");
console.log("warn is fn:", typeof consoleModule.warn === "function");
console.log("info is fn:", typeof consoleModule.info === "function");
console.log("debug is fn:", typeof consoleModule.debug === "function");
console.log("assert is fn:", typeof consoleModule.assert === "function");
console.log("clear is fn:", typeof consoleModule.clear === "function");
console.log("count is fn:", typeof consoleModule.count === "function");
console.log("countReset is fn:", typeof consoleModule.countReset === "function");
console.log("dir is fn:", typeof consoleModule.dir === "function");
console.log("dirxml is fn:", typeof consoleModule.dirxml === "function");
console.log("group is fn:", typeof consoleModule.group === "function");
console.log("groupCollapsed is fn:", typeof consoleModule.groupCollapsed === "function");
console.log("groupEnd is fn:", typeof consoleModule.groupEnd === "function");
console.log("table is fn:", typeof consoleModule.table === "function");
console.log("time is fn:", typeof consoleModule.time === "function");
console.log("timeEnd is fn:", typeof consoleModule.timeEnd === "function");
console.log("timeLog is fn:", typeof consoleModule.timeLog === "function");
console.log("trace is fn:", typeof consoleModule.trace === "function");
console.log("profile is fn:", typeof consoleModule.profile === "function");
console.log("profileEnd is fn:", typeof consoleModule.profileEnd === "function");
console.log("timeStamp is fn:", typeof consoleModule.timeStamp === "function");

const c = new consoleModule.Console(process.stdout, process.stderr);
console.log("c is obj:", typeof c === "object");
console.log("c.log is fn:", typeof c.log === "function");
console.log("c.error is fn:", typeof c.error === "function");
console.log("c.warn is fn:", typeof c.warn === "function");
console.log("c.info is fn:", typeof c.info === "function");
console.log("c.debug is fn:", typeof c.debug === "function");
consoleModule.log("node:console log works");
consoleModule.info("node:console info works");
consoleModule.debug("node:console debug works");
c.log("Console instance log works");
c.info("Console instance info works");
