// An object literal's property *names* have to survive a collection.
//
// A `kiln_shape` is `calloc`ed, never freed and never traced -- ADR 0021 wants a shape identity
// globally stable and never recycled -- but `shape->keys[]` holds interned key pointers, and an
// object-literal key is a heap block. Nothing marked them, so a key whose only remaining
// reference was a shape got purged from the weak intern table and swept while the shape still
// named it. The next object built through that shape read its property name from bytes that had
// since been handed to something else, and the name ran on past a NUL that no longer existed.
//
// It was a wrong answer rather than a refusal, which is the expensive kind. `{"mode":"..."}` came
// back as `{"modeeFile":"..."}` -- the name running into `writeFile`, an adapter-owned string that
// had been given the freed bytes -- with every *value* intact. A natively compiled server answered
// a quarter of its requests with the keys of one route and the values of another, and a benchmark
// reported `"samplesle"` where it had written `"samples"`.
//
// Fixed on main by pinning a key as it enters a shape *and* marking `object->shape->keys` when
// the object is marked. This probe is the case that the pin alone did not cover: pinning the
// transition key fixed a server returning a quarter of its responses with the wrong keys and left
// this one corrupting, which is what showed that the shape's keys have to be traced and not only
// pinned. It is kept because it reproduces through a different route than
// `array-iterator-gc.js` -- an ordinary object literal built after asynchronous host work.
//
// Three ingredients, all needed, which is why this probe is shaped the way it is: an object
// literal whose keys appear nowhere else (a name that is also read as a property is interned
// statically and pinned, so it never corrupted); enough host work to make the collector run and
// to churn the strings that reuse the bytes; and the literal built *after* that, through a shape
// created before it.
const { readFile, stat, writeFile } = require("fs/promises");

// A plain relative name rather than `os.tmpdir()`: the native backend refuses that member, and a
// probe that cannot be compiled cannot compare the two engines. Written and removed by this
// probe alone, and its two runs are sequential, so the name needs no uniquing. It is left
// behind rather than unlinked: `fs/promises.unlink` is not a member the adapter publishes, and
// the probe's job is the object literal below, not a tour of the filesystem API. `.gitignore`
// covers it.
const scratch = ".kiln-shape-keys-probe.tmp";
const payload = "shape keys survive collection\n".repeat(100);

async function main() {
  const rounds = [];
  let size = 0;
  for (let i = 0; i < 80; i += 1) {
    await writeFile(scratch, payload);
    const content = await readFile(scratch, "utf8");
    const info = await stat(scratch);
    if (info.size !== content.length) throw new Error("size mismatch");
    size = content.length;
    rounds.push(i);
  }
  console.log(JSON.stringify({
    alpha: "one", bravo: 2, charlie: [3], delta: { echo: true },
    foxtrot: rounds.length, golf: String(size),
  }));
}

main();
