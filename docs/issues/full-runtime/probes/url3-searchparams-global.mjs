// The URLSearchParams global (issue 05): held back natively until the interpreter has it.
const p = new URLSearchParams({ q: "hello world", n: 1 }); p.sort(); console.log(String(p), p.has("q"), p.getAll("n"));
for (const [k, v] of new URLSearchParams("a=1&b=2")) console.log(k, v);
