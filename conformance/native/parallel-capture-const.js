// kiln-native: profile=native-release parallelism=1,2,max
//
// The parallel/sequential differential harness (.scratch/parallel-builtins issue 02), tier 1
// (issue 05): callbacks reading captured bindings the program writes exactly once. The call
// site reads each once on the realm thread, checks it is a number, and hands it to every chunk.
// `x => x * rate` with `const rate = 1.2` is issue 05's acceptance case and runs in parallel;
// the reassigned `let` does not (`captures-reassigned` in `--explain-parallel`), and answers
// the same either way.
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

const xs = [];
for (let i = 0; i < 17000; i++) xs.push(i - 8500);
const rate = 1.2;
check("const", xs.parallelMap(x => x * rate), xs.map(x => x * rate));
const scale = 0.5;
const offset = -3;
check("two-consts", xs.parallelMap(x => x * scale + offset - scale), xs.map(x => x * scale + offset - scale));
let bias = 7;
check("let-once", xs.parallelMap((x, i) => x + bias * i), xs.map((x, i) => x + bias * i));
let bump = 1;
bump = 2;
check("let-reassigned", xs.parallelMap(x => x + bump), xs.map(x => x + bump));
const nan = NaN;
check("capture-nan", xs.parallelMap(x => x + nan), xs.map(x => x + nan));
const negZero = -0;
check("capture-negzero", xs.parallelMap(x => x * negZero), xs.map(x => x * negZero));
const inf = -Infinity;
check("capture-inf", xs.parallelMap(x => x / inf), xs.map(x => x / inf));
