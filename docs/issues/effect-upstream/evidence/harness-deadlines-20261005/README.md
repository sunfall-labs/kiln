# Deadline adapter contract, 2026-10-05

`check-effect-harness-contract.py` completed successfully against actual pinned
Vitest 5.0.3, Node 26.8.1, Bun 1.4.2, kiln interpreter, and kiln native.
All five original fixture records agree: hung cleanup fails; earlier cleanup
still runs; a beforeEach with its own deadline does not consume the test body's
deadline; a synchronous body overrun fails; throwing undefined fails. Three
failures are intentional. The verifier checks exact inventory, ordered case
outcomes, and one complete 5/2/3/0 summary, rather than trusting exit status.

The separate runner unit suite passed 28/28. Full logs and Vitest JSON are beside
this document. This is focused harness evidence, not the repository gate or a
complete upstream Effect run. The real ClusterWorkflowEngine replay is separate.

Reproduction (from this repository, with a prepared owned dependency workspace):

```sh
KILN_NATIVE_PROVIDER_MANIFEST=<provider-manifest> KILN_JOBS=1 \
  python scripts/check-effect-harness-contract.py \
  --workspace <workspace> --kiln <frozen-clock-kiln> --native
python -m unittest scripts/effect_upstream_test.py
```

Inputs:

* Harness SHA-256: `6f06bd17422ee1c290ad68cf2ef37fa8a475aa937a675c7101bbf416fc3f9f27`.
* Frozen kiln SHA-256: `e7a06733713ec8acfb7e3cc981ad78b1d881befa57b6d4ab455c3ed33f5139a7`, source `faff5879` (clock candidate, before the newer helper/Array changes).
* Native executable SHA-256: `785b793a0b70cbad4e6a36aaf73a6fa8536f4f53ae834936df00e68ce08466a1`.
* Provider manifest SHA-256: `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`.
* Pinned Vitest runner chunk SHA-256: `950469eb28cec3faa51f30c3203e1ab3099b54a52e3282b05f388930e7769717`.

The contract follows the installed primary source: `withTimeout` installs the
deadline before invoking the callback and checks elapsed time on fulfillment;
`callTestHooks` catches each finished callback independently and executes the
stack in reverse; context `onTestFinished` applies the configured hook timeout.
Node's resolved hook default is 10,000 ms in Vitest's configuration resolver.
Upstream test-body deadlines remain unchanged at their declared values, with
5,000 ms as the default. No source test or corpus classification was changed.

The raw native executable remains in scratch rather than in Git. These changes
are a local milestone awaiting the final combined-source gate and main push.
