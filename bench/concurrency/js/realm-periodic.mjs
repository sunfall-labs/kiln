// One realm per task, launched with KILN_REALMS. No module-scope shared state crosses realms.
// The runner checks every realm's output and exit status before recording a timing.
let ticks = 0;
function wake() {
  ticks += 1;
  if (ticks < 10) {
    setTimeout(wake, 1);
  } else {
    console.log(ticks);
  }
}
setTimeout(wake, 1);
