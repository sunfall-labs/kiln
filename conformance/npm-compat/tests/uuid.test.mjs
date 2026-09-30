// kiln eval smoke test (authored for kiln, not from porffor).
import {v3, v5, NIL, parse, stringify, validate, version} from 'uuid';
console.log(v5('example.com', v5.DNS));
console.log(v3('example.com', v3.DNS));
console.log(NIL);
console.log(stringify(parse(v5('example.com', v5.DNS))));
console.log(validate('6ec0bd7f-11c0-43da-975e-2a8ad9ebae0b'));
console.log(version(v5('example.com', v5.DNS)));
