// kiln eval smoke test (authored for kiln, not from porffor).
import { create, all } from 'mathjs/number';

const math = create(all);
console.log(math.evaluate('2 * (3 + 4)'));
console.log(math.sqrt(16));
console.log(math.factorial(5));
console.log(math.gcd(12, 18), math.lcm(4, 6));
console.log(math.parse('x^2').toString());
console.log(math.combinations(5, 2));
