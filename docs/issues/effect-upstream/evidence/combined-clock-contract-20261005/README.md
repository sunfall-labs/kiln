# Combined-source clock and deadline contract

The combined CLI (`8ac442d8270d0f76ea7933acef5c9812b24f1d9a1f537fbd98f25e7e43515867`, production code `79a05698`) passes the system-epoch sanity check and all five expected deadline/cleanup outcomes against pinned Vitest under Node, Bun, the interpreter and **native-release**. The fixture intentionally has two passing and three failing cases; exact inventories, outcomes and summary counts are checked, rather than interpreting its nonzero exit as a broken runtime.

The previous namespace candidate CLI (`7584f62cbcd80bfdeac802263d4bea539f5e8237ae1edb9a1c505a8e8ef004ba`) is a negative control. Although invoked with `--clock wall`, it returned synthetic epoch `1577836800001` and the updated verifier rejected it before the fixtures ran. A corrected adapter alone cannot certify real deadlines on that CLI. The epoch check rejects this observed failure; the separate fixture checks deadline and cleanup behavior.

The verifier now checks the interpreter's requested wall clock against the parent system epoch, retaining its sample/window, and explicitly builds the native contract fixture with `native-release`. No upstream source, test budget or expected outcome is changed. `old-cli-negative.log`, `verifier.log`, the oracle, individual runtime/build logs, source fixtures, epoch receipt and artifact digests are retained here. Executables are excluded; the receipt records the native executable digest and unchanged ABI-82 provider manifest digest.

This proves the clock/deadline adapter contract on the combined source, not complete upstream Effect or gate acceptance. The old synthetic-clock Channel/Hash diagnostics must not be counted as real-deadline passes.
