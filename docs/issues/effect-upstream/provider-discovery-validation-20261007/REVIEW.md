# Installed provider discovery validation

The frozen runtime candidate can automatically select separate Full and FullGui ABI 82 packages. With provider overrides unset, the actual native corpus runner matched all four GUI probes that lacked capabilities in the earlier landing gate, plus the regex probe. Two retained binaries also matched the interpreter byte for byte and exited zero.

This is packaging and selected-probe evidence, not a successful full gate, Node comparison, or complete Effect result. The compiler source corresponds to review commit 2e19ddc6 (later commits add evidence). Exact identities, commands, terminal statuses, and full logs are retained here. Package archives and executables remain in the owned scratch installation; the receipt records their content hashes.

The initial Full packaging attempt is rejected because a diagnostic edit changed realm.rs during packaging. The source was restored against the clean review tree, all 587 tracked crate inputs matched, and final Full and FullGui packaging passed pre/post source checks. Only those final packages were installed and validated. The incident assessment and restoration receipt are retained.
