const u = new URL("http://user:pw@example.com:8080/a/b?x=1&y=2#frag");
for (const k of ["href","origin","protocol","username","password","host","hostname","port","pathname","search","hash"]) console.log(k, JSON.stringify(u[k]));
console.log("searchParams.get:", u.searchParams.get("y"), "| toString:", String(u), "| json:", JSON.stringify(u));
console.log("in:", "protocol" in u, "| keys:", Object.keys(u).length, "| proto names:", Object.getOwnPropertyNames(Object.getPrototypeOf(u)).length);
