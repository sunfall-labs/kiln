import { count, limit, bump } from "./counter.ts";

console.log(count, limit);
bump();
bump();
console.log(count, limit);
