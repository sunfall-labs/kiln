// A timer suspension keeps the async frame and its captured value live while the next turn
// allocates enough short-lived strings to cross the nursery pressure threshold. The checksum
// consumes each value immediately, so stress mode does not retain a giant batch array while the
// frame is suspended. It checks that retained frame state survives every await and that reclaimed
// allocations do not corrupt later values. No wall-clock result is part of the contract.
async function main() {
  const retained = { marker: "retained-across-timers" };
  let checksum = 0;
  for (let turn = 0; turn < 24; turn += 1) {
    await new Promise((resolve) => setTimeout(resolve, 0));
    for (let index = 0; index < 256; index += 1) {
      const value = "value:" + turn + ":" + index + ":012345678901234567890123456789";
      checksum += value.length;
    }
    if (retained.marker !== "retained-across-timers") throw new Error("lost retained frame");
  }
  console.log(retained.marker + ":" + checksum);
}

main();
