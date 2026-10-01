// kiln eval smoke test (authored for kiln, not from porffor).
import { createParser } from 'eventsource-parser';
const events = [];
const parser = createParser({ onEvent: e => events.push(e.data) });
parser.feed('data: hello\n\ndata: world\n\n');
console.log(JSON.stringify(events));
const evts2 = [];
const p2 = createParser({ onEvent: e => evts2.push(e.event + ':' + e.data) });
p2.feed('event: ping\ndata: 1\n\n');
console.log(JSON.stringify(evts2));
p2.reset();
p2.feed('data: after-reset\n\n');
console.log(JSON.stringify(evts2));
