let keepRunning = false;
let immediateTurns = 0;
let zeroTimerTurns = 0;

function spinImmediate() {
  immediateTurns++;
  if (keepRunning) setImmediate(spinImmediate);
}

function spinZeroTimer() {
  zeroTimerTurns++;
  if (keepRunning) setTimeout(spinZeroTimer, 0);
}

setTimeout(() => {
  // First let the loop sit idle, then keep both zero-delay lanes continuously busy. The
  // positive timeout must fire in both cases, and must still make progress under the load.
  keepRunning = true;
  setImmediate(spinImmediate);
  setTimeout(spinZeroTimer, 0);
  setTimeout(() => {
    keepRunning = false;
    if (immediateTurns === 0 || zeroTimerTurns === 0) {
      throw new Error(`timer lanes starved: immediate=${immediateTurns}, timeout=${zeroTimerTurns}`);
    }
    console.log("pass");
  }, 25);
}, 5);
