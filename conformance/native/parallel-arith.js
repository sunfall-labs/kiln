// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02), tier 0:
// arithmetic callbacks that qualify for the pool. Each runs through `parallelMap` and `map`,
// and the answers must be the same values element for element (`Object.is`, so `-0` and `NaN`
// count). The probe throws on a difference, so the interpreter fails it outright;
// `scripts/native.sh` builds it with the release profile -- the only one that emits the
// parallel lowering -- and runs the binary at KILN_PARALLELISM=1, 2 and the core count, each of
// which must print the interpreter's bytes. `kiln build --profile native-release
// --explain-parallel` on this file says which sites are parallel: all of them, `loop` (a pure
// numeric loop over `let` locals) included.
//
// A release module past 16 functions or 512 instructions is emitted with generic bodies only
// (`kiln-native`'s specialization limit), which has no parallel lowering at all, so each file
// in this harness stays under it: one `check` and at most seven pairs.
function check(label, p, m) {
  let ok = p.length === m.length;
  for (let i = 0; ok && i < p.length; i++) ok = i in p === i in m && Object.is(p[i], m[i]);
  const at = i => (Object.is(p[i], -0) ? "-0" : String(p[i]));
  console.log(label, p.length, at(0), at(p.length >> 1), at(p.length - 1), ok ? "same" : "DIFFERENT");
  if (!ok) throw new Error("parallelMap differs from map: " + label);
}

// Past the pool's sequential floor (16,384), so every parallelism above 1 really splits it.
const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i * 0.75 - 6000);
check("affine", xs.parallelMap(x => x * 2 + 1), xs.map(x => x * 2 + 1));
check("index", xs.parallelMap((x, i) => x - i * 0.5), xs.map((x, i) => x - i * 0.5));
check("index-only", xs.parallelMap((x, i) => i % 7), xs.map((x, i) => i % 7));
check("bits", xs.parallelMap((x, i) => (x | 0) ^ (i << 3) ^ (x >>> 1) ^ ~i), xs.map((x, i) => (x | 0) ^ (i << 3) ^ (x >>> 1) ^ ~i));
check("pow", xs.parallelMap(x => (x / 6000) ** 3 - x / 7), xs.map(x => (x / 6000) ** 3 - x / 7));
check("branch", xs.parallelMap(x => { if (x < 0) return -x; return x * 3; }), xs.map(x => { if (x < 0) return -x; return x * 3; }));
check("loop", xs.parallelMap(x => { let s = 0; for (let k = 0; k < 8; k++) s += x * k; return s; }), xs.map(x => { let s = 0; for (let k = 0; k < 8; k++) s += x * k; return s; }));
