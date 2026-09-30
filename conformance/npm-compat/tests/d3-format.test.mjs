// kiln eval smoke test (authored for kiln, not from porffor).
import { format, formatPrefix } from 'd3-format';
console.log(format('$,.2f')(1234.5));
console.log(format('.0%')(0.25));
console.log(format('$,.2f')(-3.5));
console.log(formatPrefix(',.0', 1e6)(2500000));
console.log(format('05d')(42));
