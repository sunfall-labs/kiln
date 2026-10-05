# Bounded generic native tier: Sharding evidence

This is a single candidate replay, not an acceptance result. The candidate changes native-release
optimization for bounded, frame-free, direct-callable generic function bodies in large modules.
The selected Sharding input is the unchanged upstream `cluster/Sharding` suite (66 cases) from
Effect pin `67ba4e46a11ccda0b6761578bfd22c04ae00167`; each case retained the original 5,000 ms
deadline.

The candidate compiler source is commit `7461336fd18ea428ac66d3b8587622709f61f35c`. Its release
CLI SHA-256 is `8c2a4ada35219f76444d21f28c8aa4041d8720a416e09ad561a5b9dc0f4254f2`. The AOT binary
SHA-256 is `b4bd44aaebbfb84a3a9fad324555f23843c63e849c410d39fcd140443c9f8877` (221,405,464 bytes).
The native provider manifest is `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; its archive is
`61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d` (ABI 82).

The candidate build took 1,269.585 s: 40.681 s emission, 1,225.719 s compile, and 0.388 s link.
It emitted 347,616,844 bytes of C, 331,629,656 bytes of application objects, and a 221,405,464-byte
executable. The prior exact Sharding baseline in `baseline-sharding-cases.json` used pushed source
`411030258c7637c52b059d777b8fac04ada4fc01`, CLI SHA-256
`d0830baef65b01c9ccfeb626984e44cf863418a7008596188feb29822949ea88`, and built in 1,074.819 s
with a 267,324,824-byte binary. The candidate therefore cost 194.766 s (18.1%) more to compile and
produced a binary 45,919,360 bytes (17.2%) smaller. These are separate single builds, not repeated
compile-time samples.

The candidate passed 61/66 cases and retained five original 5,000 ms timeouts. Compared with the
baseline's 60/66 and six timeouts, it fixed exactly `Sharding > defects durable messages when no
entity ever registers`; the other five timeout cases stayed the same, with no new timeout. Runner
wall time was 52.81 s. The runner rebuilt the same AOT artifact with 12.029 s emission and cached
object, prelude, runtime, launcher, and final executable artifacts; its AOT binary hash matched the
build above. This was one quiet replay only, so it does not establish a stable throughput gain.

Cache inspection found 165 mixed-tier generated-C module/chunk records totaling about 150.8 MB of
source payload in this workspace. Across those records there were 5,114 emitted O0 conditional
sites. These counts are raw cache-record observations; they do not identify unique functions or
measure the amount of O1 support code. The build report's total C/object sizes above are the
authoritative graph-wide size figures. In particular, this experiment does not demonstrate that
O1 is limited to a small set of application function bodies: generated support code is compiled
alongside selected bodies, and its size can scale with the module.

The full build, replay report, complete native case log, and build log are retained beside this
note. The tier remains experimental: one fewer timeout is a concrete result, but the additional
compile cost is material and the unchanged 66-case replay needs repetition before any performance
claim.
