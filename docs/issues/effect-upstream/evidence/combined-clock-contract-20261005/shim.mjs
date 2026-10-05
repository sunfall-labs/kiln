import * as harness from './harness.mjs';
import {register} from './cases.mjs';
register(harness);
await harness.run();
