// An application-shaped probe, not a member probe: the first twenty lines of an ordinary CLI.
// Read a file, count words, hash it, format a date, a custom Error subclass, Set algebra. On
// 2026-09-19 the native build of this died at `s.union` with "undefined is not a function" --
// a silent undefined on the instance read path, which the surface inventory (prototype reads)
// could not see. Every line here is something a real program does in its first minute.
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { randomUUID, createHash } = require("node:crypto");

class ConfigError extends Error {
  constructor(message) {
    super(message);
    this.name = "ConfigError";
  }
}

const src = fs.readFileSync(__filename, "utf8");
const lines = src.split(/\r?\n/);
const words = new Map();
for (const w of src.toLowerCase().match(/[a-z]+/g) ?? []) words.set(w, (words.get(w) ?? 0) + 1);
const top = [...words.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0])).slice(0, 3);
const d = new Date(Date.UTC(2026, 8, 19, 12, 30));
d.setUTCHours(0, 0, 0, 0);
const id = randomUUID();
const sha = createHash("sha256").update(src).digest("hex").slice(0, 8);
console.log(`file: ${path.basename(__filename)} (${lines.length} lines, ${Buffer.byteLength(src)} bytes)`);
console.log("top words:", top.map(([w, n]) => `${w.padEnd(8)}${String(n).padStart(3)}`).join(" | "));
console.log("day:", d.toISOString(), "| date:", d.toDateString(), "| utc:", d.toUTCString(), "| weekday:", d.getUTCDay());
console.log("home set:", typeof os.homedir() === "string", "| uuid ok:", /^[0-9a-f-]{36}$/.test(id), "| sha ok:", sha.length === 8);
try { JSON.parse("{bad"); } catch (e) { console.log("caught:", e.name, e instanceof SyntaxError); }
try { throw new ConfigError("missing key"); } catch (e) { console.log("custom:", e.name, e.message, e instanceof ConfigError, e instanceof Error); }
class Quiet extends Error {}
console.log("subclass name:", new Quiet("q").name, "| proto name:", Error.prototype.name);
const s = new Set([1, 2, 3]);
console.log("set ops:", [...s.union(new Set([4]))].length, s.isSubsetOf(new Set([1, 2, 3, 4])), [...s.intersection(new Set([2, 3, 9]))].join(","), [...s.difference(new Set([1]))].join(","));
console.debug("debug:", { ok: true });
console.assert(1 === 1, "never printed");
