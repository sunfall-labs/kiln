// A signal reaches the entry module while its top-level await is pending and a guard timer is
// armed. The listener is unreferenced: it keeps nothing alive by itself, but while the guard
// does, node delivers the signal as soon as it arrives. A virtual-clock drive with only
// unreferenced work pending used to jump straight to the guard, so the signal lost the race.
const race = (label, start) =>
  new Promise((resolve) => {
    const guard = setTimeout(() => resolve(label + " timeout"), 3000);
    start((value) => {
      clearTimeout(guard);
      resolve(label + " " + value);
    });
  });

console.log(await race("signal", (done) => {
  process.once("SIGUSR2", () => done("SIGUSR2"));
  setTimeout(() => process.kill(process.pid, "SIGUSR2"), 20);
}));
