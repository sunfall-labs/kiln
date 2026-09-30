// kiln eval smoke test (authored for kiln, not from porffor).
import { timeFormat, timeParse, utcFormat, utcParse, isoFormat } from 'd3-time-format';

const d = new Date(Date.UTC(2026, 8, 30, 12, 34, 56));
console.log('utcFormat:', utcFormat('%Y-%m-%d %H:%M:%S')(d));
console.log('timeFormat:', timeFormat('%B %d, %Y')(d));
const parsed = utcParse('%Y-%m-%d')( '2026-09-30');
console.log('utcParse iso:', parsed.toISOString());
const p2 = timeParse('%m/%d/%Y')('09/30/2026');
console.log('timeParse parts:', p2.getFullYear(), p2.getMonth(), p2.getDate());
console.log('isoFormat:', isoFormat(d));
console.log('weekday:', utcFormat('%A')(d));
console.log('roundtrip:', utcFormat('%Y-%m-%dT%H:%M:%SZ')(utcParse('%Y-%m-%dT%H:%M:%SZ')('2026-09-30T12:00:00Z')));
