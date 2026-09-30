// kiln eval smoke test (authored for kiln, not from porffor).
import { sum, mean, extent, max, min, bin } from 'd3-array';
console.log(sum([1, 2, 3, 4]), mean([1, 2, 3, 4]), max([1, 9, 3]), min([1, 9, 3]));
console.log(JSON.stringify(extent([3, 1, 2])));
console.log(JSON.stringify(bin().thresholds(2)([1, 2, 3, 4]).map(b => b.length)));
