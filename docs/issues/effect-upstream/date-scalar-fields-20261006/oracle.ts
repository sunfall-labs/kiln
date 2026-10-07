const methods = ["getUTCFullYear", "getUTCMonth", "getUTCDate", "getUTCDay", "getUTCHours", "getUTCMinutes", "getUTCSeconds", "getUTCMilliseconds", "getYear"];
const times = [NaN, Infinity, -Infinity, 0, -0, 1, -1, 999, -999, 1000, -1000, 59999, -59999, 60000, -60000, 86399999, -86399999, 86400000, -86400000, 8.64e15, -8.64e15, 8.64e15+1, -8.64e15-1];
let seed = 1;
for (let i = 0; i < 2000; i++) { seed = (seed * 1664525 + 1013904223) >>> 0; times.push(Math.trunc((seed / 4294967296 * 2 - 1) * 8.64e15)); }
const read = () => times.map(t => { const d = new Date(t); return methods.map(name => d[name]()); });
const expected = read();
await import("../../../../crates/semantics/kiln-builtins/src/date.ts");
const actual = read();
for (let i=0; i<times.length; i++) for(let j=0;j<methods.length;j++) if (!Object.is(expected[i][j], actual[i][j])) throw new Error(`${times[i]} ${methods[j]} expected ${expected[i][j]} got ${actual[i][j]}`);
for (const name of methods) { let thrown=false; try { Date.prototype[name].call({}); } catch(e) { thrown=e instanceof TypeError; } if(!thrown) throw new Error(name+" must reject ordinary object"); }
console.log(`matched ${times.length * methods.length} fields and ${methods.length} receiver checks`);
