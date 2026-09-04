import { floor } from "./harness.mjs";

// The empty benchmark, whose whole job is to be subtracted.
//
// A floor has to include everything a benchmark pays before its own workload runs: process
// start, module load, and the harness's clock probe -- which is not free, because classifying a
// clock in a runtime whose clock does not move means doing work and watching it not move. An
// empty *file* would miss the last of those and make every externally timed row too slow by the
// cost of the probe.
floor();
