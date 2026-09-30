import { inspect } from 'node:util';
console.log(inspect('plain'), inspect("a'b"), inspect('a"b'), inspect("a'b\"c"));
