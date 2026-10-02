const tick = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms));
async function main() {
  const got = [];
  for await (const v of [tick(1, "a"), tick(1, "b"), tick(1, "c"), "d", "e"]) {
    got.push(v);
    for (let i = 0; i < 60000; i++) { const junk = { x: i, y: [i], z: { i } }; if (junk.x < 0) got.push(junk); }
  }
  console.log("got:", got.join(","));
}
main();
