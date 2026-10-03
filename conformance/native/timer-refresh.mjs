// `Timeout.prototype.refresh()`: the same timer restarted with its own delay from now, keeping
// its callback, arguments and referenced state. It returns the Timeout, restarts an interval's
// current period, re-arms a timeout that already ran -- after the fact, or from inside its own
// callback, the keep-alive pattern -- and does nothing to a cleared timer. Every comparison is
// between deadlines at least 20 ms apart, so node's real clock and kiln's virtual one order them
// the same way.
const log = (...parts) => console.log(...parts);

// 1. Refreshing pushes the deadline back: `slow` is due at 50, but refreshed at 30 it is due at
// 80, behind `other` at 70. The callback and its arguments are the ones it was armed with.
const slow = setTimeout((a, b) => log("slow fired", a, b), 50, "x", "y");
setTimeout(() => log("other fired at 70"), 70);
setTimeout(() => {
  log("refresh returns the handle", slow.refresh() === slow);
  log("same handle after refresh", slow.hasRef());
}, 30);

// 2. Refreshing a timeout that already ran arms it again, with the same callback and arguments:
// it ran at 15, is refreshed at 45, and runs a second time at 60.
const done = setTimeout((a, b) => log("done fired", a, b), 15, "p", "q");
setTimeout(() => log("refreshed after it ran, returns the handle", done.refresh() === done), 45);

// 3. Refreshing a cleared timer does not arm it either.
const cleared = setTimeout(() => log("cleared timer fired"), 20);
clearTimeout(cleared);
cleared.refresh();

// 4. The referenced state survives: an unreferenced timer stays unreferenced (it is due at 1000
// and the program does not wait for it), a referenced one is still referenced.
const quiet = setTimeout(() => log("unref'd timeout fired"), 1000);
quiet.unref();
setTimeout(() => {
  quiet.refresh();
  log("unref survives refresh", quiet.hasRef());
}, 5);
const loud = setTimeout(() => log("loud fired"), 140);
loud.unref().ref();
setTimeout(() => {
  loud.refresh();
  log("ref survives refresh", loud.hasRef());
}, 100);

// 5. An interval restarts its current period: ticks at 40, then refreshed at 60 the next one is
// due at 100 rather than 80, behind the marker at 90.
let ticks = 0;
const interval = setInterval(() => {
  ticks++;
  log("tick", ticks);
  if (ticks === 2) clearInterval(interval);
}, 40);
setTimeout(() => interval.refresh(), 60);
setTimeout(() => log("marker at 90"), 90);

// 6. A keep-alive: a timeout that refreshes itself from inside its own callback, a bounded
// number of times, runs at 20, 40 and 60 and is not re-armed after the third.
let keep = 0;
const alive = setTimeout(() => {
  keep++;
  log("keep-alive run", keep);
  if (keep < 3) alive.refresh();
}, 20);

// 7. A timeout cleared after it ran, with its callback dropped, does not come back.
let revived = 0;
const gone = setTimeout(() => revived++, 10);
setTimeout(() => {
  clearTimeout(gone);
  gone.refresh();
}, 30);

setTimeout(() => {
  log("ticks", ticks, "revived", revived);
}, 500);
