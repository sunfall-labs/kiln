# Prepared iterator call facts baseline

The gate on `f0eefa7eedede3dfdc40a6e71815e28226b318cb` found a stale whole-program facts baseline after `bdf2941f1d22cc701ba84e229573297c241e426f` replaced iterator argument arrays with prepared calls.

Regenerated all 31 targets (14 benchmarks, 17 bundles) with the gate-built `kiln-facts` binary. A structural comparison of old and new JSON verified all 108 changed functions: exactly 176 `NewArray` sites disappeared, no sites were added, and every surviving site ID and all 23 changed `returned_sites` lists were remapped by the exact number of preceding removed instructions. Function metadata, module metadata, function counts, module counts, and closed/open-world classifications are unchanged.

`kiln-facts --check` then passed all 31 targets. This is a reviewed analysis snapshot update; it does not classify the other failed gate steps as passing or establish completion of the Effect suite.
