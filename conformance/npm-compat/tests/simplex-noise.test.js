// kiln eval smoke test (authored for kiln, not from porffor).
const { createNoise2D } = require('simplex-noise');

// Compare deterministic permutations across engines; ambient randomness is not parity evidence.
const noise2D = createNoise2D(mulberry32(98765));
console.log(noise2D(0, 0).toFixed(6));
console.log(noise2D(0.5, -1.25).toFixed(6));
console.log(noise2D(0, 0) === noise2D(0, 0));

function mulberry32(seed) {
  return function () {
    seed |= 0;
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const seeded = createNoise2D(mulberry32(12345));
console.log(seeded(3.3, 7.7).toFixed(6));
console.log(seeded(3.3, 7.7) === seeded(3.3, 7.7));
