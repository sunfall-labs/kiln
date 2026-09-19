// A data CLI: parse CSV, aggregate, format a report. fs, path, os, process, Date, sorting, padding.
const fs = require("node:fs"); const path = require("node:path"); const os = require("node:os");
const dir = fs.mkdtempSync(path.join(os.tmpdir(), "kiln-csv-"));
const file = path.join(dir, "sales.csv");
fs.writeFileSync(file, ["region,item,qty,price,when", "north,widget,3,9.99,2026-01-05", "south,gadget,1,24.5,2026-01-07", "north,gadget,2,24.5,2026-02-01", "west,widget,10,9.99,2026-02-14", "south,widget,4,9.99,2026-03-03"].join("\n") + "\n");
const rows = fs.readFileSync(file, "utf8").trim().split("\n").slice(1).map((line) => { const [region, item, qty, price, when] = line.split(","); return { region, item, qty: Number(qty), price: parseFloat(price), when: new Date(when + "T00:00:00Z") }; });
const byRegion = new Map();
for (const r of rows) { const acc = byRegion.get(r.region) ?? { total: 0, items: new Set(), last: null }; acc.total += r.qty * r.price; acc.items.add(r.item); if (!acc.last || r.when > acc.last) acc.last = r.when; byRegion.set(r.region, acc); }
const report = [...byRegion].sort(([a], [b]) => a.localeCompare(b)).map(([region, a]) => `${region.padEnd(6)} ${a.total.toFixed(2).padStart(8)} ${[...a.items].sort().join("+").padEnd(14)} ${a.last.toISOString().slice(0, 10)} ${a.last.toUTCString()}`);
console.log(report.join("\n"));
const months = Object.groupBy(rows, (r) => r.when.getUTCMonth() + 1);
console.log(Object.entries(months).map(([m, rs]) => `m${m}:${rs.length}`).join(" "), "| max qty:", Math.max(...rows.map((r) => r.qty)), "| json:", JSON.stringify(rows[0], ["region", "qty"]));
console.log("exists:", fs.existsSync(file), "| stat size > 0:", fs.statSync(file).size > 0, "| dir entries:", fs.readdirSync(dir).length, "| ext:", path.extname(file), "| rel:", path.relative(dir, file));
fs.rmSync(dir, { recursive: true, force: true });
console.log("cleaned:", !fs.existsSync(dir), "| argv0 is string:", typeof process.argv[0], "| cwd:", typeof process.cwd(), "| env has PATH:", "PATH" in process.env);
process.exitCode = 0;
