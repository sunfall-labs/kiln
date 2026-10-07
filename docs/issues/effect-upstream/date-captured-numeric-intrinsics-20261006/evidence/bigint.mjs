await import(process.argv[2]);
for (const [method, receiver] of [["setTime", new Date(0)], ["setUTCDate", new Date(NaN)]]) {
  try { console.log(method, Date.prototype[method].call(receiver, 1n)); }
  catch (error) { console.log(method, error.name); }
}
