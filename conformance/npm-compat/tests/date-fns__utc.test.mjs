// kiln eval smoke test (authored for kiln, not from porffor).
import {UTCDate} from '@date-fns/utc';
const d = new UTCDate('2024-01-02T03:04:05Z');
console.log(d.getUTCFullYear(), d.getUTCMonth(), d.getUTCDate(), d.getUTCHours());
console.log(d.toISOString());
const local = new UTCDate(2024, 5, 15);
console.log(local.getUTCFullYear(), local.getUTCMonth(), local.getUTCDate());
console.log(new UTCDate(0).toISOString());
