import * as fakeTimers from '@sinonjs/fake-timers';
import {installVi,vi} from './harness.mjs';
let clock;
installVi({
  useFakeTimers(options){clock=fakeTimers.install(options);return vi;},
  useRealTimers(){if(clock){clock.uninstall();clock=undefined;}return vi;},
  advanceTimersByTimeAsync(ms){return clock.tickAsync(ms);},
  advanceTimersByTime(ms){clock.tick(ms);return vi;},
  runAllTimersAsync(){return clock.runAllAsync();}
});
