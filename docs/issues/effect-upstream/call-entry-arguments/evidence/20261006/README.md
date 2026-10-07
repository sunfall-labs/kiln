# Call-entry argument ownership validation

This change moves each call's full incoming argument vector into the active frame or generator state, cloning only the formal prefix into local slots. It avoids cloning the complete argument vector and an extra Vec allocation while preserving surplus arguments for the arguments object, rest parameters, and generator prologues.

- Base source commit: 9c95cb5630030bc6c9a23166eb29ead331414e6e (includes VM run-guard commit bfe6faac).
- Validation command: mbx test -p kiln-interpreter --offline --jobs=1 -- --test-threads=1.
- Toolchain: Rust 1.97.1 and mbx 1.22.0; shared integration mbx cache; MBX_GC_AUTO=false, KILN_JOBS=1, RAYON_NUM_THREADS=1.
- Result: 289 passed, 0 failed, 1 ignored; the ordinary arguments, rest-arguments, and generator-prologue regressions passed. The test process exited 0.
- Raw log: interpreter-tests.log.gz; uncompressed log SHA-256 ff7e83fb701e84e8c1b3f51f893416450635dd2611ee90f778d9f60fec5c0489.
- Source/test patch SHA-256: 9e191bebf17b130688ee7113b288ab702e3bfa227763233a2c7bc48794e64b99.

An accidental second test command began with Rust 1.99.0 during evidence-file creation because a shell here-document was unquoted. I terminated that process while it was compiling a dependency; it did not complete and is excluded from the result above. No release CLI or upstream corpus was built or rerun.
