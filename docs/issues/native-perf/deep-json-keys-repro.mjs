const q = String.fromCharCode(34);
function nest(n, distinct) {
  let s = String(1);
  for (let i = n - 1; i >= 0; i--) {
    const k = distinct ? ("k" + i) : "d";
    s = "{" + q + k + q + ":" + s + "}";
  }
  return s;
}
for (const n of [8, 20, 31, 32, 33, 40, 64, 200]) {
  for (const distinct of [false, true]) {
    const src = nest(n, distinct);
    let out;
    try { out = JSON.stringify(JSON.parse(src)); } catch (e) { out = "THREW " + e.message; }
    console.log(n, distinct ? "distinct" : "same", out === src ? "roundtrip" : ("DIFFERS " + out.slice(0, 70)));
  }
}
