// Bun's test API owns ESM namespace spies; ordinary property replacement cannot
// mutate an ESM namespace. Capture only the mock operations used by our adapter.
import { vi } from 'bun:test';
globalThis.__kilnEffectHarnessHost = {
  spy: {
    fn: vi.fn,
    spyOn: vi.spyOn,
    clearAllMocks: vi.clearAllMocks,
    resetAllMocks: vi.resetAllMocks,
    restoreAllMocks: vi.restoreAllMocks,
  },
};
