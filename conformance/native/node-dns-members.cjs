// What `require("node:dns")` publishes, and what functions answer.
const dns = require("node:dns");

const names = Object.getOwnPropertyNames(dns).sort();
console.log("names length:", names.length);
for (const name of names) {
  console.log(name, typeof dns[name]);
}

console.log("default self-link:", dns.default === dns);
console.log("ADDRCONFIG:", dns.ADDRCONFIG);
console.log("ALL:", dns.ALL);
console.log("V4MAPPED:", dns.V4MAPPED);
console.log("NODATA:", dns.NODATA);
console.log("NOTFOUND:", dns.NOTFOUND);

console.log("getDefaultResultOrder:", dns.getDefaultResultOrder());
dns.setDefaultResultOrder("ipv4first");
console.log("getDefaultResultOrder after set:", dns.getDefaultResultOrder());
dns.setDefaultResultOrder("verbatim");

console.log("getServers:", dns.getServers().join(","));

dns.lookup("localhost", (err, address, family) => {
  console.log("lookup localhost:", err === null, address, family);
});

dns.lookup("127.0.0.1", (err, address, family) => {
  console.log("lookup 127.0.0.1:", err === null, address, family);
});

dns.lookup("localhost", { all: true }, (err, addresses) => {
  console.log("lookup all:", err === null, Array.isArray(addresses), addresses.length);
});

dns.lookupService("127.0.0.1", 80, (err, hostname, service) => {
  console.log("lookupService:", err === null, hostname, service);
});

dns.resolve4("localhost", (err, addresses) => {
  console.log("resolve4:", err === null, addresses.join(","));
});

dns.resolve6("localhost", (err, addresses) => {
  console.log("resolve6:", err === null, addresses.join(","));
});

dns.reverse("127.0.0.1", (err, hostnames) => {
  console.log("reverse:", err === null, hostnames.join(","));
});

const resolver = new dns.Resolver();
console.log("resolver is obj:", typeof resolver === "object");
console.log("resolver.getServers:", resolver.getServers().join(","));

console.log("promises is obj:", typeof dns.promises === "object");
console.log("promises.lookup is fn:", typeof dns.promises.lookup === "function");
