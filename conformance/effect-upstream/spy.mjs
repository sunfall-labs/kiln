import * as spy from '@vitest/spy';
import {installVi} from './harness.mjs';
installVi({...spy,resetModules(){throw new Error('HARNESS_UNSUPPORTED: vi.resetModules');},importActual(){throw new Error('HARNESS_UNSUPPORTED: vi.importActual');}});
