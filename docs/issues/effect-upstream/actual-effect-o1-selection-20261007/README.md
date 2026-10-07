# Actual Effect O1 selection (2026-10-07)

Clean combined source `c06c78329eb9c30f2c8ab44007b5054bb4a1dc2b` built successfully in 143.51 seconds. Frozen CLI SHA-256: `af80860eda702b4d6d6742ca15e5b23013fe65649e759b0078aba1ffdef73d27`.

The original upstream Sharding entry was checked through the native-release build's own emission, stopping before C compilation/linking. This completed in 38.65 seconds. The actual internal Effect module now emits fn120 (the computed evaluate method proven separately) without O0/optnone attributes; fn50 remains selected, and fn123 remains O0. The prior attempt had an identical old payload and retained fn120 O0. The new payload SHA-256 is `8660857e06985caaccc6e28f7bf14ab997b2bfba3ce1216f0a9d62a079e0dc9f`. Full declaration attributes and record/content digests are preserved in the proof JSON; the compressed C was round-trip verified against its recorded digest.

The analysis admission cap is now 1,024 function prototypes, accommodating this module's 739 prototypes within the original instruction, caller, edge and depth bounds. The emitted O1 body limit remains 512. Runtime method dispatch remains dynamic. The cap-1,024 source passed six planner tests and all 37 emitted-C integration tests, archived separately under `computed-key-o1-proof-20261007`. The newly frozen CLI also retained exact Node/Bun output parity on the Array.fill probe.

This proves actual upstream optimization selection, not native speed or full Effect acceptance. A symbol-retaining native Sharding build was launched after this check; its completion and original-deadline replay require separate evidence. No upstream test source, assertion, case mode or five-second deadline changed.
