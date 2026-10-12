// Growing private strings, immutable snapshots, UTF-16 widening and exceptional exits.
function build(n: number): string {
  let s = "prefix:";
  for (let i = 0; i < n; i++) {
    const piece = i % 3 === 0 ? "é" : i % 3 === 1 ? "😀" : "\ud800";
    s += piece;
  }
  return s;
}
const text = build(600);
let hash = 0;
for (let i = 0; i < text.length; i++) hash = (Math.imul(hash, 31) + text.charCodeAt(i)) | 0;
console.log(JSON.stringify([text.length, hash, text.slice(0, 15), text.slice(-8)]));
function snapshots() {
  let s = "";
  const saved: string[] = [];
  for (let i = 0; i < 20; i++) { saved.push(s); s += "x"; }
  return saved.join("|");
}
console.log(snapshots());
function retained() {
  let s = "";
  let before = "";
  for (let i = 0; i < 20; i++) { before = s; s += "y"; }
  return [before, s];
}
console.log(JSON.stringify(retained()));
function callback() {
  let s = "";
  const saved: string[] = [];
  function suffix() { saved.push(s); return "z"; }
  for (let i = 0; i < 20; i++) s += suffix();
  return saved.join("|");
}
console.log(callback());
function throwing() {
  let s = "";
  try {
    for (let i = 0; i < 40; i++) {
      if (i === 17) throw new Error("stop");
      s += "a";
    }
  } catch { s += "caught"; }
  return s;
}
console.log(throwing());
function conditional() {
  let s = "";
  for (let i = 0; i < 50; i++) { if (i % 2) s += "b"; else s += "c"; }
  return s;
}
console.log(conditional());
function coerced() {
  let s = "";
  const item = { [Symbol.toPrimitive]() { return "Q"; } };
  for (let i = 0; i < 30; i++) s += item;
  return s;
}
console.log(coerced());

function freezeAtExit() {
  let s = "literal";
  const initial = s;
  for (let i = 0; i < 300; i++) s += "x";
  const saved = s;
  s += "tail";
  return [initial, saved.length, s.length, saved.slice(-6)];
}
console.log(JSON.stringify(freezeAtExit()));
function allocatingSuffix(i: number): string {
  const record = { value: i };
  return String(record.value).padStart(3, "0");
}
function allocatingLoop() {
  let s = "";
  for (let i = 0; i < 80; i++) s += allocatingSuffix(i);
  return s;
}
console.log(allocatingLoop());
