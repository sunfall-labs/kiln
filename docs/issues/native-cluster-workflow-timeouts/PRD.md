# Native ClusterWorkflowEngine timeouts

Status: in-progress

The frozen full-survey CLI `07ef070f4450438e565a6c20cb594ce24639a5532358c40b6156ebf99a03cb9e` completed all 47 registered cases: 45 passed and two hit their original five-second deadlines. The failures are bounded child concurrency across suspended activity replays and keeping a parent suspended past the activity interrupt retry budget. Bun completed the same suite 47/47 in 2.92 seconds; native execution took 40.39 seconds under concurrent survey compilation.

The candidate executable SHA256 was `fb1ddddf65cd8944492d2eb24d2dc623340fbd6f007e73fd136dacc3a7a79e0b`. It is losslessly archived under the survey output and its restored digest was checked before compression. Full per-file envelopes and raw case logs are retained here.

The old CLI predates the verified prepared Promise.resolve/await and Math.sqrt guards. A targeted run with source `2ef02041` and CLI `de776124500fa715493ade1b03c5742a0c8dc77b0b558466632b3163dcbfee8e` is compiling with one worker and unchanged deadlines. This will distinguish an already repaired optimization gap from a remaining workflow or performance failure. Load-sensitive timing must be checked under controlled execution; increasing deadlines or skipping cases does not prove the issue fixed.
