import { addDays, format, differenceInDays, parseISO } from "date-fns";
const start = parseISO("2020-01-01T00:00:00.000Z");
const later = addDays(start, 45);
console.log(format(later, "yyyy-MM-dd"));
console.log(differenceInDays(later, start));
