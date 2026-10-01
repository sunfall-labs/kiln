// kiln eval smoke test (authored for kiln, not from porffor).
import { scaleLinear, scaleBand, scaleOrdinal, scaleLog } from 'd3-scale';
console.log(scaleLinear().domain([0, 10]).range([0, 100])(5));
console.log(scaleBand().domain(['a', 'b']).range([0, 100]).bandwidth());
console.log(scaleOrdinal().domain(['x', 'y']).range(['red', 'blue'])('y'));
console.log(scaleLog().domain([1, 100]).range([0, 2])(10));
console.log(scaleLinear().domain([0, 10]).range([0, 100]).invert(75));
