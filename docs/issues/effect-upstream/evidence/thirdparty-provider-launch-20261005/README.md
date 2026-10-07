# Third-party process startup attribution: forced FullGui provider

This is a focused diagnostic for the uniform process-startup increase in the 2026-10-05 gate's
third-party timing lane. It is not a baseline update or a performance acceptance run.

The gate log selected all 12 routine bundles and measured 49.39–67.57 ms per fresh process,
compared with recorded baselines of 5.04–24.35 ms. Every bundle regressed by a nearly constant
amount. The gate's `thirdparty.sh` times each native pass around a fresh `subprocess.run([bin])`,
so ELF loader startup is included in every sample.

The gate's explicit provider manifest was FullGui. The manifest's capabilities were
`realm,reg-exp,big-int,json,temporal,intl,parallel,http,ffi,gui`, while the generated tiny control
program required only `realm,reg-exp,big-int,json`. `kiln-native::number_provider::discover_for`
returns `load_manifest` immediately when `KILN_NATIVE_PROVIDER_MANIFEST` (or its legacy alias) is
set, before selecting the smallest variant for the program's demand. Thus the override applies the
GUI provider to every native bundle.

## Paired binary diagnostic

Using the immutable gate CLI, I built the same `console.log("ok")` module twice in native-release
mode. Both builds reported the same four required capabilities and reused the same generated-C,
app-object, prelude-object, and runtime-object records. The only changed input was the provider
manifest: FullGui versus the non-GUI Full provider. The FullGui provider was ABI 82 and included
GTK4/Wayland link arguments; the headless Full provider was ABI 82 and had no GUI link arguments.

- CLI SHA-256: `ce4844eb1e84a593c0d052de922540abcd30ed27f6c6a520f175125fa67e8b50`
- Gate source snapshot: `fb9c274063c9b559460e3dac3d93b392c02c6e9c966b143dd47d7882a12663ee`
- FullGui manifest SHA-256: `5eaa7d77a4e17c0d376c48b773a08870de12a0768fb3c3d69f3129fdb6e5fd2e`
- Headless manifest SHA-256: `b03c48a204de3f911b424ce62b7d96a88ef595de0f99217cba8c34caf6c5dd6e`
- FullGui test binary SHA-256: `b58dc6e22a3f57e5df48d947729673e4266ae1a17e9ff72ad42b7b0e563bf87a`
- Headless test binary SHA-256: `9b5176254734d7a720cde5ac32588e9d4a73cc99e0b53285e6e7fb2641be610d`

`readelf -d` reports 13 direct GUI shared-library dependencies for FullGui, including
`libgtk-4`, `libwayland-client`, Pango, HarfBuzz, Cairo, Vulkan, Graphene, GIO, GObject, and GLib.
`ldd` expands these to roughly 60 transitive GTK/GStreamer/X11/Wayland/font/graphics libraries.
The headless binary has only four direct dependencies: `libm`, `libgcc_s`, `libc`, and the ELF loader.
The full `ldd` outputs are retained in `ldd.txt`.

I ran 24 alternating fresh-process launches of each binary pinned to CPU 15 and checked status,
stdout, and stderr for every run. Median process times were 94.40 ms for FullGui and 10.18 ms for
the headless variant. This diagnostic ran while other gate lanes were active; its absolute values
are not a quiet benchmark claim. The raw 48 measurements are in `launch-pair.json`. The large
paired difference plus the direct dependency inspection support the conclusion that unnecessary
GUI library loading explains the gate's shared startup penalty.

The appropriate next measurement uses demand-based provider selection with both explicit provider
manifest variables unset for the third-party lane. Do not alter the baseline or skip bundles.
