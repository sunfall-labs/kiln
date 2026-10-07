globalThis.__badRuns = (globalThis.__badRuns || 0) + 1;
throw new Error("bad module");
