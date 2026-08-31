import diagnostics from 'node:diagnostics_channel';
import { debuglog, format } from 'node:util';

const channel = diagnostics.channel('kiln.compat');
const tracing = diagnostics.tracingChannel('kiln.compat.trace');
if (channel.hasSubscribers !== false || tracing.hasSubscribers !== false) throw new Error('subscribers');
channel.publish({ ignored: true });
let ran = false;
tracing.start.runStores({}, function (value) { ran = this.ok && value === 42; }, { ok: true }, 42);
if (!ran) throw new Error('runStores');
if (format('%s:%d %%', 'port', 3000) !== 'port:3000 %') throw new Error('format');
if (typeof debuglog('kiln') !== 'function') throw new Error('debuglog');
console.log('pass');
