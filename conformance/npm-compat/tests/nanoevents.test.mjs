// kiln eval smoke test (authored for kiln, not from porffor).
import { createNanoEvents } from 'nanoevents';

const ee = createNanoEvents();
const seen = [];
const unbind = ee.on('msg', (d) => seen.push(d));
ee.emit('msg', 'hi');
ee.emit('msg', 'there');
unbind();
ee.emit('msg', 'ignored');
console.log(seen.join(','));
console.log(ee.events.msg.length);
