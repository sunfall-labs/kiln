import * as spy from '@vitest/spy';
import {installVi} from './harness.mjs';
const host = globalThis.__kilnEffectHarnessHost;
delete globalThis.__kilnEffectHarnessHost;
installVi({...spy,...host?.spy,resetModules(){throw new Error('HARNESS_UNSUPPORTED: vi.resetModules');},importActual(){throw new Error('HARNESS_UNSUPPORTED: vi.importActual');}});
