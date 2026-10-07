# Provider demand selection and GUI link startup evidence

This evidence checks two separate claims: an installed kiln can select the smallest provider
variant for a non-GUI program without a manifest override, and GUI link dependencies account for
the observed process-startup cost. It is diagnostic evidence for provider selection, not the full
third-party benchmark gate.

## Installed-layout demand selection

The private installed layout used for the check was:

```text
installed-layout/
  bin/kiln
  lib/kiln/kiln-native-provider.json
  lib/kiln/libkiln_native_provider_ffi.a
  lib/kiln/kiln-native-provider.full-gui.json
  lib/kiln/libkiln_native_provider_ffi.full-gui.a
```

The `bin/kiln` file was the frozen installed CLI with SHA-256
`f68949f7e9f66848e5e6294fa972a06be7f42378b1cbd1ccecf35bc82f00348a`. Both
`KILN_NATIVE_PROVIDER_MANIFEST` and `KILN_NATIVE_NUMBER_PROVIDER_MANIFEST` were unset. The
command was:

```sh
env -u KILN_NATIVE_PROVIDER_MANIFEST -u KILN_NATIVE_NUMBER_PROVIDER_MANIFEST \
  KILN_JOBS=1 RAYON_NUM_THREADS=1 \
  installed-layout/bin/kiln --one-shot build \
  .scratch/effect-perf-sandbox/conformance/thirdparty/ms.bundle.js \
  --profile native-release \
  --output installed-layout/ms-demand.bin
```

The frozen CLI resolves its executable path and checks the adjacent `../lib/kiln` manifests.
The build reported requirements `realm,reg-exp,big-int,json,intl` and selected a provider with
`realm,reg-exp,big-int,json,temporal,intl,parallel,http,ffi` (Full, not FullGui). This is the
expected `smallest_covering` choice for this demand. The resulting executable has four dynamic
dependencies (`libm`, `libgcc_s`, `libc`, and the ELF loader), no GUI libraries. Its stdout is
byte-identical to the retained FullGui executable, SHA-256
`896757587675d609d01a50358df94afa72fc1465e2974115898c081d1eb1fc13`.

The FullGui and Full build logs are retained under `raw/`. Both builds used the same source module
symbol and emitted C size (52,445,854 bytes), but they were separate CLI builds and should not be
treated as an object-identical timing pair. The Full demand-selection build took 178.426 seconds;
this is not a process-startup measurement.

## Link-only experiment

The retained app objects are `raw/app-00.o` through `raw/app-31.o`, with two shared runtime
objects and one launcher object. `link-only/full-provider.bin` linked those inputs against the
Full archive. `ms-full-gui.bin` is the original FullGui executable built from those same object
payloads with the original FullGui link recipe. The Full image has four `DT_NEEDED` entries; the
FullGui image has seventeen, including Wayland, GTK, Pango, Harfbuzz, Pixbuf, Cairo, Vulkan,
Graphene, Gio, GObject, and GLib. Both print identical stdout.

`raw/launch-samples.csv` is an initial warmup-and-alternating sample for a different pair:
`full-provider.bin` and `gui-export-rooted.bin`. The latter used `--as-needed` and the extra
`--undefined=kiln_gui_init` linker root. Keep it as a GUI retention control only; do not report it
as the FullGui-versus-Full latency comparison. The quiet sample of the exact `ms-full-gui.bin` and
`full-provider.bin` pair is pending.

## Files and provenance

`raw/` contains the build reports, provider manifests, output bytes, initial launch CSV, exact
link commands, dynamic dependency listings, and SHA-256 inventory. Large executable and object
files are intentionally not copied into this evidence commit; their hashes and source paths are
recorded in `raw/sha256.txt`.

The installed-layout CLI and the original FullGui executable were frozen artifacts from the
effect integration worktree. The CLI and executable hashes, provider manifest/archive hashes,
and input bundle hash are recorded in `raw/sha256.txt` and `raw/input-sha256.txt`. The exact
source revision embedded in the frozen CLI was not independently recoverable from the binary;
the binary hash is the authority for this experiment.
