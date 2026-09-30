// kiln eval smoke test (authored for kiln, not from porffor).
import { format, addDays, differenceInDays, isWeekend, startOfMonth, subMonths } from 'date-fns';
const d = new Date(2024, 0, 15);
console.log(format(d, 'yyyy-MM-dd'));
console.log(format(addDays(d, 5), 'yyyy-MM-dd'));
console.log(differenceInDays(new Date(2024, 1, 1), d));
console.log(isWeekend(new Date(2024, 0, 13)), isWeekend(d));
console.log(format(startOfMonth(d), 'yyyy-MM-dd'), format(subMonths(d, 1), 'yyyy-MM-dd'));
