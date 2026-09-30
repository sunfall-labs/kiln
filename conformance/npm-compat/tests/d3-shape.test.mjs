// kiln eval smoke test (authored for kiln, not from porffor).
import { line, arc, pie, area } from 'd3-shape';
console.log(line()([[0, 0], [1, 1], [2, 0]]));
console.log(arc()({ innerRadius: 0, outerRadius: 10, startAngle: 0, endAngle: Math.PI / 2 }));
console.log(JSON.stringify(pie()([1, 2, 3]).map(a => a.value)));
console.log(area()([[0, 0], [1, 1]]));
console.log(line().curve ? 'has-curve' : 'no-curve');
