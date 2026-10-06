# Native private elements on constructor-returned Proxies

The native Proxy payload now owns a lazy private-elements object. Compiler-generated private brand/member writes, `DefinePrivate`, private reads, and private brand lookups use that sidecar, so they do not consult the Proxy target or invoke public traps. Public properties continue through the Proxy algorithms. Heap tracing and graph-transfer edge inspection include the sidecar.

The focused fixture exercises a derived constructor returning a Proxy, private field/method/getter/setter access, an ordinary computed string beginning with `@@private:` still reaching public traps, private access on a revoked Proxy through the class prototype, and the expected TypeError for a foreign receiver. The existing `class-field-proxy-descriptor.js` fixture was also compared directly with Node; it checks public class-field descriptors and symbol keys.

Validation source was based on `184b7a2a`. The final native CLI was built with Rust 1.97.1 using `mbx build --locked --offline -p kiln-cli --release`, with one Cargo/Kiln/Rayon worker and a 4 GiB systemd memory cap. Build log: `.scratch/proxy-derived-vm-validation/logs/native-cli-build-final.log`. The immutable CLI SHA-256 is `cc8b8560c35d5a4b4d28667fef1136b16cfdf8fd69635ab246addb1b27a92822` at `.scratch/proxy-derived-vm-validation/toolchain/kiln-cc8b8560c35d5a4b4d28667fef1136b16cfdf8fd69635ab246addb1b27a92822`.

The provider manifest SHA-256 is `4b68a2200f148e19ff7fcd146cacd5c8ffb546692d67e52478fb3b266c19621b`; its archive SHA-256 is `61c1ef04a85b93ec98e561e320d4c056f6c2a07e788a25f71f1273f4ef5c2f6d` (ABI 82). The updated conformance fixture SHA-256 is `fecd37eb499835b4fda5c8148fc4541c3ab5421e5ba0d8006923042ccf2e959c`.

Direct Node/native stdout comparisons passed in ordinary and `KILN_NATIVE_GC_STRESS=1` modes for both fixtures. `scripts/native.sh conformance/native/proxy-private-class-elements.js` reported 1 matched, 0 refused, 0 diverged. Raw stdout/stderr and commands are preserved under `.scratch/proxy-derived-vm-validation/logs/`, including `proxy-private-ordinary-final3.log`, `class-field-ordinary-final.log`, `node-native-gc-stress-final.log`, and `native-runner-proxy-ordinary-final.log`. `cargo fmt --all -- --check` and `git diff --check` passed.

The IR currently lowers private `#x in object` to ordinary `BinaryOp::In` plus a brand comparison. That operation still follows Proxy `has`/revocation behavior; this change does not claim to fix private-in semantics on Proxies.
