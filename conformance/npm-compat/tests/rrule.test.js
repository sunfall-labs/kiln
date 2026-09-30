// kiln eval smoke test (authored for kiln, not from porffor).
const {RRule, RRuleSet, rrulestr} = require('rrule');
const r = new RRule({freq: RRule.WEEKLY, count: 3, dtstart: new Date(Date.UTC(2024, 0, 1, 9, 0, 0))});
console.log(r.all().map(d => d.toISOString().slice(0, 10)).join(','));
const r2 = rrulestr('DTSTART:20240101T000000Z\nRRULE:FREQ=DAILY;COUNT=2');
console.log(r2.all().length, r2.all()[1].toISOString().slice(0, 10));
const set = new RRuleSet();
set.rrule(new RRule({freq: RRule.DAILY, count: 5, dtstart: new Date(Date.UTC(2024, 0, 1))}));
set.exdate(new Date(Date.UTC(2024, 0, 3)));
console.log(set.all().length);
console.log(r.toString().slice(0, 40));
