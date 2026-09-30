// kiln eval smoke test (authored for kiln, not from porffor).
import { interpolate, interpolateRgb, interpolateNumber, interpolateRound } from 'd3-interpolate';
console.log(interpolate(0, 100)(0.5));
console.log(interpolateRgb('red', 'blue')(0.5));
console.log(interpolateNumber(10, 20)(0.25));
console.log(interpolateRound(0, 10)(0.55));
console.log(interpolate('a1b', 'a9b')(0.5));
