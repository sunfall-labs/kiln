# RxJS latency after correct timer deadlines

The Linux native-release RxJS baseline moves from 18.68 ms to 24.35 ms because wall timers now arm from their call time. This is a deliberate correctness-related latency change. The probe uses sequential delays of 3, 2 and 1 ms plus a four-tick interval; its whole-process time includes those delays and process startup. It is not a steady-state throughput measurement.

A control burns 30 ms of synchronous work before requesting a 10 ms timeout. Node waits the requested delay. Main's provider fires too early; a provider built from main with only `d6f45b73` applied waits correctly. The same main-generated RxJS program was linked against the original, timer-only and integrated providers. Only the provider differs in these pairs. Timer-only and integrated providers both increase latency; main and the candidate with the integrated provider have similar latency. Paired diagnostics ran under bounded concurrent work and establish the cause, not an optimization percentage.

The accepted baseline uses 63 output-validated fresh-process samples and three warmups, with the pinned mise toolchain, frozen integration CLI and source-matched provider. Its source tree was clean. Only the RxJS bundle row changes; other bundle measurements remain intact. The normal regression threshold remains 25% and 3 ms. The next integration gate must independently pass against this reviewed baseline.

[Evidence](rxjs-timer-deadlines-2026-10-04.json) retains all paired samples, provider digests, the control source/results and before/after baseline provenance.
